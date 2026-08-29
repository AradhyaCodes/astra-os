//! Authentication and independent resource-lock policy for Aaru-OS.
//!
//! Password hashes are persistent. Authentication attempts, the login session,
//! and temporary resource grants are deliberately process-local.

use crate::error::AaruError;
use crate::filesystem::ResourceId;
use argon2::password_hash::{rand_core::OsRng, PasswordHash, SaltString};
use argon2::{Argon2, PasswordHasher, PasswordVerifier};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

pub const MAX_AUTH_ATTEMPTS: u8 = 3;
const MINIMUM_PASSWORD_LENGTH: usize = 8;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PersistentSecurity {
    login_password_hash: Option<String>,
    resource_lock_hashes: BTreeMap<ResourceId, String>,
    /// Aaru-level locks on **host** resources, keyed by stable canonical path.
    /// These are Aaru access gates only — they do not touch Windows ACLs.
    #[serde(default)]
    host_lock_hashes: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Default)]
struct SecuritySession {
    authenticated: bool,
    failed_login_attempts: u8,
    login_locked_out: bool,
    authenticated_resources: BTreeSet<ResourceId>,
    failed_resource_attempts: BTreeMap<ResourceId, u8>,
    locked_out_resources: BTreeSet<ResourceId>,
    authenticated_host_paths: BTreeSet<String>,
    failed_host_attempts: BTreeMap<String, u8>,
    locked_out_host_paths: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AuthenticationStatus {
    pub configured: bool,
    pub authenticated: bool,
    pub failed_attempts: u8,
    pub remaining_attempts: u8,
    pub locked_out: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceAuthenticationStatus {
    pub path: String,
    pub authenticated_boundary_id: Option<ResourceId>,
    pub remaining_boundaries: usize,
}

#[derive(Debug, Clone)]
pub struct SecurityManager {
    persistent: PersistentSecurity,
    session: SecuritySession,
}

impl SecurityManager {
    pub fn new(persistent: PersistentSecurity) -> Self {
        Self {
            persistent,
            session: SecuritySession::default(),
        }
    }

    pub fn status(&self) -> AuthenticationStatus {
        AuthenticationStatus {
            configured: self.persistent.login_password_hash.is_some(),
            authenticated: self.session.authenticated,
            failed_attempts: self.session.failed_login_attempts,
            remaining_attempts: MAX_AUTH_ATTEMPTS
                .saturating_sub(self.session.failed_login_attempts),
            locked_out: self.session.login_locked_out,
        }
    }

    pub fn configure_login(&mut self, password: &str) -> Result<AuthenticationStatus, AaruError> {
        if self.persistent.login_password_hash.is_some() {
            return Err(AaruError::CredentialsAlreadyConfigured);
        }
        validate_password(password)?;
        self.persistent.login_password_hash = Some(hash_password(password)?);
        self.session.authenticated = true;
        self.session.failed_login_attempts = 0;
        Ok(self.status())
    }

    pub fn login(&mut self, password: &str) -> Result<AuthenticationStatus, AaruError> {
        if self.session.login_locked_out {
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        let hash = self
            .persistent
            .login_password_hash
            .as_deref()
            .ok_or(AaruError::CredentialsNotConfigured)?;

        if verify_password(hash, password)? {
            self.session.authenticated = true;
            self.session.failed_login_attempts = 0;
            return Ok(self.status());
        }

        self.session.authenticated = false;
        self.session.failed_login_attempts = self.session.failed_login_attempts.saturating_add(1);
        if self.session.failed_login_attempts >= MAX_AUTH_ATTEMPTS {
            self.session.login_locked_out = true;
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        Err(AaruError::AuthenticationFailed)
    }

    pub fn logout(&mut self) {
        self.session.authenticated = false;
        self.session.authenticated_resources.clear();
        self.session.failed_resource_attempts.clear();
        self.session.locked_out_resources.clear();
        self.session.authenticated_host_paths.clear();
        self.session.failed_host_attempts.clear();
        self.session.locked_out_host_paths.clear();
    }

    /// Verify a password against the stored login hash **without** mutating the
    /// failed-attempt counter or session. Used to confirm identity for
    /// non-authentication actions such as `almanac logout`.
    pub fn verify_login_password(&self, password: &str) -> bool {
        self.persistent
            .login_password_hash
            .as_deref()
            .map(|hash| verify_password(hash, password).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn require_login(&self) -> Result<(), AaruError> {
        if self.session.authenticated {
            Ok(())
        } else {
            Err(AaruError::AuthenticationRequired)
        }
    }

    pub fn add_resource_lock(
        &mut self,
        resource_id: ResourceId,
        password: &str,
    ) -> Result<(), AaruError> {
        self.require_login()?;
        if self
            .persistent
            .resource_lock_hashes
            .contains_key(&resource_id)
        {
            return Err(AaruError::InvalidArgument(
                "resource is already locked".to_string(),
            ));
        }
        validate_password(password)?;
        self.persistent
            .resource_lock_hashes
            .insert(resource_id, hash_password(password)?);
        self.session.authenticated_resources.remove(&resource_id);
        Ok(())
    }

    pub fn remove_resource_lock(
        &mut self,
        resource_id: ResourceId,
        password: &str,
    ) -> Result<(), AaruError> {
        self.require_login()?;
        self.verify_resource_password(resource_id, password)?;
        self.persistent.resource_lock_hashes.remove(&resource_id);
        self.session.authenticated_resources.remove(&resource_id);
        self.session.failed_resource_attempts.remove(&resource_id);
        self.session.locked_out_resources.remove(&resource_id);
        Ok(())
    }

    pub fn authenticate_next_boundary(
        &mut self,
        boundaries: &[ResourceId],
        password: &str,
    ) -> Result<Option<ResourceId>, AaruError> {
        self.require_login()?;
        let boundary = boundaries.iter().copied().find(|resource_id| {
            self.is_resource_locked(*resource_id)
                && !self.session.authenticated_resources.contains(resource_id)
        });
        let Some(resource_id) = boundary else {
            return Ok(None);
        };
        self.verify_resource_password(resource_id, password)?;
        self.session.authenticated_resources.insert(resource_id);
        self.session.failed_resource_attempts.remove(&resource_id);
        Ok(Some(resource_id))
    }

    pub fn require_boundaries(&self, boundaries: &[ResourceId]) -> Result<(), ResourceId> {
        boundaries
            .iter()
            .copied()
            .find(|resource_id| {
                self.is_resource_locked(*resource_id)
                    && !self.session.authenticated_resources.contains(resource_id)
            })
            .map_or(Ok(()), Err)
    }

    pub fn is_resource_locked(&self, resource_id: ResourceId) -> bool {
        self.persistent
            .resource_lock_hashes
            .contains_key(&resource_id)
    }

    pub fn retain_existing_resources(&mut self, existing: &BTreeSet<ResourceId>) {
        self.persistent
            .resource_lock_hashes
            .retain(|resource_id, _| existing.contains(resource_id));
        self.session
            .authenticated_resources
            .retain(|resource_id| existing.contains(resource_id));
    }

    pub(crate) fn locked_resource_ids(&self) -> BTreeSet<ResourceId> {
        self.persistent
            .resource_lock_hashes
            .keys()
            .copied()
            .collect()
    }

    pub(crate) fn copy_resource_locks(&mut self, pairs: &[(ResourceId, ResourceId)]) {
        for (source_id, copied_id) in pairs {
            if let Some(hash) = self.persistent.resource_lock_hashes.get(source_id).cloned() {
                self.persistent
                    .resource_lock_hashes
                    .insert(*copied_id, hash);
                self.session.authenticated_resources.remove(copied_id);
            }
        }
    }

    pub(crate) fn persistent(&self) -> &PersistentSecurity {
        &self.persistent
    }

    // ------------------------------------------------------------------
    // Host-resource locks
    //
    // These are Aaru-level access gates keyed by a stable canonical host path.
    // They do NOT encrypt anything, do NOT touch Windows ACLs, and do NOT make
    // the folder inaccessible to other Windows programs.
    // ------------------------------------------------------------------

    pub fn is_host_locked(&self, canonical_id: &str) -> bool {
        self.persistent.host_lock_hashes.contains_key(canonical_id)
    }

    pub fn add_host_lock(&mut self, canonical_id: &str, password: &str) -> Result<(), AaruError> {
        self.require_login()?;
        if self.persistent.host_lock_hashes.contains_key(canonical_id) {
            return Err(AaruError::InvalidArgument(
                "this host resource is already locked".to_string(),
            ));
        }
        validate_password(password)?;
        self.persistent
            .host_lock_hashes
            .insert(canonical_id.to_string(), hash_password(password)?);
        self.session.authenticated_host_paths.remove(canonical_id);
        Ok(())
    }

    pub fn remove_host_lock(
        &mut self,
        canonical_id: &str,
        password: &str,
    ) -> Result<(), AaruError> {
        self.require_login()?;
        self.verify_host_password(canonical_id, password)?;
        self.persistent.host_lock_hashes.remove(canonical_id);
        self.session.authenticated_host_paths.remove(canonical_id);
        self.session.failed_host_attempts.remove(canonical_id);
        self.session.locked_out_host_paths.remove(canonical_id);
        Ok(())
    }

    /// First applicable ancestor id that is locked and not yet authenticated.
    pub fn require_host_boundaries(&self, ancestor_ids: &[String]) -> Result<(), String> {
        ancestor_ids
            .iter()
            .find(|id| {
                self.persistent.host_lock_hashes.contains_key(*id)
                    && !self.session.authenticated_host_paths.contains(*id)
            })
            .map_or(Ok(()), |id| Err(id.clone()))
    }

    /// Authenticate the first still-locked ancestor id with `password`.
    pub fn authenticate_host_boundary(
        &mut self,
        ancestor_ids: &[String],
        password: &str,
    ) -> Result<Option<String>, AaruError> {
        self.require_login()?;
        let target = ancestor_ids
            .iter()
            .find(|id| {
                self.persistent.host_lock_hashes.contains_key(*id)
                    && !self.session.authenticated_host_paths.contains(*id)
            })
            .cloned();
        let Some(id) = target else {
            return Ok(None);
        };
        self.verify_host_password(&id, password)?;
        self.session.authenticated_host_paths.insert(id.clone());
        self.session.failed_host_attempts.remove(&id);
        Ok(Some(id))
    }

    fn verify_host_password(
        &mut self,
        canonical_id: &str,
        password: &str,
    ) -> Result<(), AaruError> {
        if self.session.locked_out_host_paths.contains(canonical_id) {
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        let hash = self
            .persistent
            .host_lock_hashes
            .get(canonical_id)
            .ok_or_else(|| {
                AaruError::InvalidArgument("this host resource is not locked".to_string())
            })?;
        if verify_password(hash, password)? {
            return Ok(());
        }
        let attempts = self
            .session
            .failed_host_attempts
            .entry(canonical_id.to_string())
            .or_default();
        *attempts = attempts.saturating_add(1);
        if *attempts >= MAX_AUTH_ATTEMPTS {
            self.session
                .locked_out_host_paths
                .insert(canonical_id.to_string());
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        Err(AaruError::AuthenticationFailed)
    }

    fn verify_resource_password(
        &mut self,
        resource_id: ResourceId,
        password: &str,
    ) -> Result<(), AaruError> {
        if self.session.locked_out_resources.contains(&resource_id) {
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        let hash = self
            .persistent
            .resource_lock_hashes
            .get(&resource_id)
            .ok_or_else(|| AaruError::InvalidArgument("resource is not locked".to_string()))?;
        if verify_password(hash, password)? {
            return Ok(());
        }

        let attempts = self
            .session
            .failed_resource_attempts
            .entry(resource_id)
            .or_default();
        *attempts = attempts.saturating_add(1);
        if *attempts >= MAX_AUTH_ATTEMPTS {
            self.session.locked_out_resources.insert(resource_id);
            return Err(AaruError::AccountLocked {
                attempts: MAX_AUTH_ATTEMPTS,
            });
        }
        Err(AaruError::AuthenticationFailed)
    }
}

impl Default for SecurityManager {
    fn default() -> Self {
        Self::new(PersistentSecurity::default())
    }
}

fn validate_password(password: &str) -> Result<(), AaruError> {
    if password.chars().count() < MINIMUM_PASSWORD_LENGTH {
        return Err(AaruError::InvalidArgument(format!(
            "password must contain at least {MINIMUM_PASSWORD_LENGTH} characters"
        )));
    }
    Ok(())
}

fn hash_password(password: &str) -> Result<String, AaruError> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|hash| hash.to_string())
        .map_err(|error| AaruError::Persistence(format!("password hashing failed: {error}")))
}

fn verify_password(hash: &str, password: &str) -> Result<bool, AaruError> {
    let parsed = PasswordHash::new(hash)
        .map_err(|_| AaruError::CorruptPersistence("invalid password hash".to_string()))?;
    Ok(Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_correct_password_and_rejects_incorrect_password() {
        let mut security = SecurityManager::default();
        security.configure_login("correct-horse").unwrap();
        security.logout();
        assert!(matches!(
            security.login("wrong-password"),
            Err(AaruError::AuthenticationFailed)
        ));
        assert!(security.login("correct-horse").unwrap().authenticated);
    }

    #[test]
    fn locks_login_after_three_failed_attempts() {
        let mut security = SecurityManager::default();
        security.configure_login("correct-horse").unwrap();
        security.logout();
        assert!(matches!(
            security.login("incorrect-1"),
            Err(AaruError::AuthenticationFailed)
        ));
        assert!(matches!(
            security.login("incorrect-2"),
            Err(AaruError::AuthenticationFailed)
        ));
        assert!(matches!(
            security.login("incorrect-3"),
            Err(AaruError::AccountLocked { attempts: 3 })
        ));
        assert!(matches!(
            security.login("correct-horse"),
            Err(AaruError::AccountLocked { attempts: 3 })
        ));
    }

    #[test]
    fn persistent_security_contains_hashes_not_plaintext() {
        let mut security = SecurityManager::default();
        security.configure_login("never-store-me").unwrap();
        let json = serde_json::to_string(security.persistent()).unwrap();
        assert!(!json.contains("never-store-me"));
        assert!(json.contains("$argon2"));
    }
}
