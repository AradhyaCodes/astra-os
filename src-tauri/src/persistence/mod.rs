//! Durable JSON snapshot persistence behind a replaceable storage abstraction.

use crate::error::AstraError;
use crate::filesystem::VirtualFileSystem;
use crate::fs_provider::HostMountRecord;
use crate::memory::MemoryRuntimeSnapshot;
use crate::process::ProcessRuntimeSnapshot;
use crate::scheduler::SchedulerRuntimeSnapshot;
use crate::security::PersistentSecurity;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Bumped to 2 in Phase 4 (host mount records + host lock hashes). Older v1
/// snapshots still load — the new fields use `#[serde(default)]`.
pub const CURRENT_SCHEMA_VERSION: u32 = 3;
const MINIMUM_SCHEMA_VERSION: u32 = 1;

/// A state file larger than this is treated as unusable — it is set aside and
/// the runtime starts fresh rather than spending minutes deserialising it on
/// every boot. Comfortably above a healthy snapshot; only a runaway import
/// gets here.
const MAX_STATE_FILE_BYTES: u64 = 96 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemSettings {
    pub values: BTreeMap<String, Value>,
}

impl Default for SystemSettings {
    fn default() -> Self {
        let mut values = BTreeMap::new();
        values.insert("theme".to_string(), Value::String("dark".to_string()));
        Self { values }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistentSnapshot {
    pub schema_version: u32,
    pub filesystem: VirtualFileSystem,
    pub security: PersistentSecurity,
    pub settings: SystemSettings,
    /// Almanac command history (passwords never enter it — prompt input is a
    /// separate IPC path).
    #[serde(default)]
    pub command_history: Vec<String>,
    /// User-approved host mounts (default mounts are recomputed at startup).
    #[serde(default)]
    pub host_mounts: Vec<HostMountRecord>,
    /// One-shot Phase 9 runtime snapshot. It is consumed and removed on the
    /// next boot; ordinary restart/shutdown snapshots always leave it empty.
    #[serde(default)]
    pub hibernate: Option<HibernateSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HibernateSnapshot {
    pub processes: ProcessRuntimeSnapshot,
    pub scheduler: SchedulerRuntimeSnapshot,
    pub memory: MemoryRuntimeSnapshot,
    pub cwd: String,
    pub ui_session: Value,
    pub almanac_session: Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResumeSession {
    pub cwd: String,
    pub ui_session: Value,
    pub almanac_session: Value,
}

impl Default for PersistentSnapshot {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            filesystem: VirtualFileSystem::new(),
            security: PersistentSecurity::default(),
            settings: SystemSettings::default(),
            command_history: Vec::new(),
            host_mounts: Vec::new(),
            hibernate: None,
        }
    }
}

pub trait PersistenceStore {
    fn load(&self) -> Result<Option<PersistentSnapshot>, AstraError>;
    fn save(&self, snapshot: &PersistentSnapshot) -> Result<(), AstraError>;
}

#[derive(Debug, Clone)]
pub struct JsonPersistence {
    path: PathBuf,
}

#[derive(Debug)]
pub struct LoadReport {
    pub snapshot: PersistentSnapshot,
    pub recovery_notice: Option<String>,
}

impl JsonPersistence {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn load_recovering(&self) -> Result<LoadReport, AstraError> {
        if let Some(size) = file_size(&self.path) {
            if size > MAX_STATE_FILE_BYTES {
                let sidelined = self.sideline_oversized_file()?;
                return Ok(LoadReport {
                    snapshot: PersistentSnapshot::default(),
                    recovery_notice: Some(format!(
                        "state file was {} MiB (over the {} MiB limit) — set aside as {} and \
                         started fresh",
                        size / (1024 * 1024),
                        MAX_STATE_FILE_BYTES / (1024 * 1024),
                        sidelined.display()
                    )),
                });
            }
        }
        if !self.path.exists() && self.backup_path().exists() {
            let snapshot = self.load()?.unwrap_or_default();
            return Ok(LoadReport {
                snapshot,
                recovery_notice: Some(
                    "primary snapshot was missing; loaded atomic backup".to_string(),
                ),
            });
        }
        match self.load() {
            Ok(Some(snapshot)) => Ok(LoadReport {
                snapshot,
                recovery_notice: None,
            }),
            Ok(None) => Ok(LoadReport {
                snapshot: PersistentSnapshot::default(),
                recovery_notice: None,
            }),
            Err(AstraError::CorruptPersistence(reason)) => {
                let quarantined = self.quarantine_corrupt_file()?;
                let snapshot = self.load()?.unwrap_or_default();
                Ok(LoadReport {
                    snapshot,
                    recovery_notice: Some(format!(
                        "{reason}; corrupt snapshot moved to {}",
                        quarantined.display()
                    )),
                })
            }
            Err(error) => Err(error),
        }
    }

    fn sideline_oversized_file(&self) -> Result<PathBuf, AstraError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let target = self
            .path
            .with_extension(format!("oversized-{timestamp}.json"));
        fs::rename(&self.path, &target).map_err(|error| {
            AstraError::Persistence(format!("could not set aside oversized state: {error}"))
        })?;
        // A stale backup would just be re-loaded on the next boot.
        let _ = fs::remove_file(self.backup_path());
        Ok(target)
    }

    fn quarantine_corrupt_file(&self) -> Result<PathBuf, AstraError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let corrupt_path = self
            .path
            .with_extension(format!("corrupt-{timestamp}.json"));
        fs::rename(&self.path, &corrupt_path).map_err(|error| {
            AstraError::Persistence(format!("could not quarantine corrupt state: {error}"))
        })?;
        Ok(corrupt_path)
    }

    fn temporary_path(&self) -> PathBuf {
        self.path.with_extension("json.tmp")
    }

    fn backup_path(&self) -> PathBuf {
        self.path.with_extension("json.bak")
    }
}

impl PersistenceStore for JsonPersistence {
    fn load(&self) -> Result<Option<PersistentSnapshot>, AstraError> {
        let source = if self.path.exists() {
            &self.path
        } else if self.backup_path().exists() {
            return read_snapshot(&self.backup_path()).map(Some);
        } else {
            return Ok(None);
        };
        read_snapshot(source).map(Some)
    }

    fn save(&self, snapshot: &PersistentSnapshot) -> Result<(), AstraError> {
        let parent = self.path.parent().ok_or_else(|| {
            AstraError::Persistence("state path has no parent directory".to_string())
        })?;
        fs::create_dir_all(parent).map_err(|error| {
            AstraError::Persistence(format!("could not create state directory: {error}"))
        })?;

        let bytes = serde_json::to_vec_pretty(snapshot)
            .map_err(|error| AstraError::Serialization(error.to_string()))?;
        let temporary = self.temporary_path();
        if temporary.exists() {
            fs::remove_file(&temporary).map_err(|error| {
                AstraError::Persistence(format!("could not remove stale temporary state: {error}"))
            })?;
        }
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| {
                AstraError::Persistence(format!("could not create temporary state: {error}"))
            })?;
        file.write_all(&bytes).map_err(|error| {
            AstraError::Persistence(format!("could not write temporary state: {error}"))
        })?;
        file.sync_all().map_err(|error| {
            AstraError::Persistence(format!("could not flush temporary state: {error}"))
        })?;
        drop(file);

        let backup = self.backup_path();
        if backup.exists() {
            fs::remove_file(&backup).map_err(|error| {
                AstraError::Persistence(format!("could not clear old state backup: {error}"))
            })?;
        }
        if self.path.exists() {
            fs::rename(&self.path, &backup).map_err(|error| {
                AstraError::Persistence(format!("could not stage existing state: {error}"))
            })?;
        }
        if let Err(error) = fs::rename(&temporary, &self.path) {
            if backup.exists() {
                let _ = fs::rename(&backup, &self.path);
            }
            return Err(AstraError::Persistence(format!(
                "could not commit new state: {error}"
            )));
        }
        if backup.exists() {
            if let Err(error) = fs::remove_file(&backup) {
                log::warn!("Could not clear committed state backup: {error}");
            }
        }
        Ok(())
    }
}

fn file_size(path: &Path) -> Option<u64> {
    fs::metadata(path).ok().map(|meta| meta.len())
}

fn read_snapshot(path: &Path) -> Result<PersistentSnapshot, AstraError> {
    let bytes = fs::read(path)
        .map_err(|error| AstraError::Persistence(format!("could not read state: {error}")))?;
    let mut snapshot: PersistentSnapshot = serde_json::from_slice(&bytes)
        .map_err(|_| AstraError::CorruptPersistence("invalid state JSON".to_string()))?;
    if snapshot.schema_version < MINIMUM_SCHEMA_VERSION
        || snapshot.schema_version > CURRENT_SCHEMA_VERSION
    {
        return Err(AstraError::CorruptPersistence(format!(
            "unsupported state schema version {}",
            snapshot.schema_version
        )));
    }
    // Forward-migrate in memory; the next save rewrites at the current version.
    snapshot.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn snapshots_round_trip_and_corruption_is_quarantined() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let mut snapshot = PersistentSnapshot::default();
        snapshot
            .filesystem
            .create_file("ROOT>Documents", "saved.txt", "durable")
            .unwrap();
        store.save(&snapshot).unwrap();

        let loaded = store.load().unwrap().unwrap();
        assert_eq!(
            loaded
                .filesystem
                .read_file("ROOT", "Documents>saved.txt")
                .unwrap(),
            "durable"
        );

        let mut corrupt = File::create(store.path()).unwrap();
        corrupt.write_all(b"not valid json").unwrap();
        drop(corrupt);
        let report = store.load_recovering().unwrap();
        assert!(report.recovery_notice.is_some());
        assert!(report
            .snapshot
            .filesystem
            .inspect("ROOT", "Documents>saved.txt")
            .is_err());
        assert!(!store.path().exists());
    }

    #[test]
    fn an_oversized_state_file_is_set_aside_and_a_fresh_snapshot_is_returned() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));

        let bloated = File::create(store.path()).unwrap();
        bloated.set_len(MAX_STATE_FILE_BYTES + 1).unwrap();
        drop(bloated);

        let report = store.load_recovering().unwrap();
        assert!(report
            .recovery_notice
            .as_deref()
            .unwrap()
            .contains("over the"));
        assert!(!store.path().exists());
        assert!(directory
            .path()
            .read_dir()
            .unwrap()
            .filter_map(Result::ok)
            .any(|entry| entry.file_name().to_string_lossy().contains("oversized-")));
    }
}
