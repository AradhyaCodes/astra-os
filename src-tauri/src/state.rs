use crate::error::AstraError;
use crate::filesystem::model::{ResourceId, ResourceType, ROOT_ID};
use crate::filesystem::path::{normalize_path, split_path};
use crate::filesystem::{
    DeleteSummary, Permissions, ResourceInfo, SearchResults, VirtualFileSystem,
};
use crate::fs_provider::host::{HostDirs, HostEntry};
use crate::fs_provider::{
    route, AstraLocation, EntryView, HostFilesystem, HostFilesystemProvider, MountView,
    ProviderKind, SearchHit, VirtualFilesystemProvider,
};
use crate::kernel::SchedulerAlgorithm;
use crate::memory::{MemoryManager, MemorySnapshot, ReplacementPolicy};
use crate::persistence::{
    HibernateSnapshot, JsonPersistence, PersistenceStore, PersistentSnapshot, ResumeSession,
    SystemSettings, CURRENT_SCHEMA_VERSION,
};
use crate::process::{
    find_builtin, resolve_host_app, HostAppResolution, LaunchReport, PcbView, Pid, ProcessManager,
    ProcessType,
};
use crate::scheduler::{ScheduleClass, Scheduler, SchedulerSnapshot, Workload};
use crate::security::{AuthenticationStatus, ResourceAuthenticationStatus, SecurityManager};
use serde::Serialize;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use zeroize::Zeroizing;

/// Upper bound on retained command history entries.
const MAX_HISTORY_ENTRIES: usize = 500;

/// Per-file ceiling for a HOST↔ASTRA copy. The virtual filesystem is held
/// entirely in memory and rewritten to a single JSON document on every
/// mutation, so a very large file would make every later command slow. Files
/// above this are skipped and reported.
pub const MAX_CROSS_COPY_FILE_BYTES: u64 = 4 * 1024 * 1024;

/// Outcome of a cross-boundary (HOST↔ASTRA) copy or transfer.
#[derive(Debug, Default, Clone)]
pub struct CrossCopySummary {
    pub created_path: String,
    pub files: u64,
    pub dirs: u64,
    /// Total entries the walk could not copy (it continues past them).
    pub skipped: usize,
    /// The first handful of `scope — reason` strings, for display.
    pub skip_details: Vec<String>,
}

impl CrossCopySummary {
    fn note_skip(&mut self, scope: &str, error: &AstraError) {
        self.skipped += 1;
        if self.skip_details.len() < 20 {
            self.skip_details.push(format!("{scope} — {error}"));
        }
    }
}

/// `HOST>alias>a>b` display string for a mount-relative path.
fn host_scope(alias: &str, relative: &[String]) -> String {
    if relative.is_empty() {
        format!("HOST>{alias}")
    } else {
        format!("HOST>{alias}>{}", relative.join(">"))
    }
}

#[derive(Debug, Clone, Copy)]
enum AccessRequirement {
    Read,
    Write,
    Execute,
}

/// An interactive prompt parked on the session by an Almanac command.
///
/// This is deliberately process-local: it is never serialised and never
/// persisted. Secrets it holds live in [`Zeroizing`] so they are wiped on drop,
/// and its [`Debug`] impl never prints them.
#[derive(Default)]
pub enum PendingPrompt {
    #[default]
    None,
    DestroyConfirm {
        path: String,
        total: u64,
    },
    LockPassword {
        path: String,
    },
    LockConfirm {
        path: String,
        first: Zeroizing<String>,
    },
    UnlockPassword {
        path: String,
    },
    LogoutPassword,
    KillLapsessionConfirm,
    TransferHostConfirm {
        cwd: String,
        from: String,
        to: String,
    },
    // ---- host variants ----
    DestroyHostConfirm {
        alias: String,
        relative: Vec<String>,
        display: String,
    },
    HostLockPassword {
        canonical_id: String,
        display: String,
    },
    HostLockConfirm {
        canonical_id: String,
        display: String,
        first: Zeroizing<String>,
    },
    HostUnlockPassword {
        canonical_id: String,
        display: String,
    },
}

impl std::fmt::Debug for PendingPrompt {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let name = match self {
            PendingPrompt::None => "None",
            PendingPrompt::DestroyConfirm { .. } => "DestroyConfirm",
            PendingPrompt::LockPassword { .. } => "LockPassword",
            PendingPrompt::LockConfirm { .. } => "LockConfirm(redacted)",
            PendingPrompt::UnlockPassword { .. } => "UnlockPassword",
            PendingPrompt::LogoutPassword => "LogoutPassword",
            PendingPrompt::KillLapsessionConfirm => "KillLapsessionConfirm",
            PendingPrompt::TransferHostConfirm { .. } => "TransferHostConfirm",
            PendingPrompt::DestroyHostConfirm { .. } => "DestroyHostConfirm",
            PendingPrompt::HostLockPassword { .. } => "HostLockPassword",
            PendingPrompt::HostLockConfirm { .. } => "HostLockConfirm(redacted)",
            PendingPrompt::HostUnlockPassword { .. } => "HostUnlockPassword",
        };
        write!(f, "PendingPrompt::{name}")
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceSecurityInfo {
    pub resource: ResourceInfo,
    pub pending_lock_boundaries: Vec<String>,
}

/// Merged virtual + host search results for `almanac lookout`.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize)]
pub struct LookoutResults {
    pub hits: Vec<SearchHit>,
    pub skipped: Vec<String>,
}

#[derive(Debug)]
pub struct SystemState {
    filesystem: VirtualFileSystem,
    security: SecurityManager,
    settings: SystemSettings,
    command_history: Vec<String>,
    persistence: JsonPersistence,
    /// Approved host mounts and the host filesystem bridge.
    host: HostFilesystem,
    /// Astra process table (in-memory only — running process state is never
    /// restored across a restart).
    processes: ProcessManager,
    /// Virtual CPU scheduler for simulated Astra processes (Phase 6). In-memory
    /// only and independent of the real Windows scheduler.
    scheduler: Scheduler,
    /// Simulated paged memory subsystem (Phase 7). In-memory only and fully
    /// separate from real Windows host memory.
    memory: MemoryManager,
    /// Outstanding interactive prompt (Almanac `destroy`/`lock`/`unlock`/`logout`).
    pending: PendingPrompt,
    /// Monotonic counter backing prompt correlation ids.
    prompt_counter: u64,
    /// UI/runtime payload consumed from a one-shot hibernate snapshot.
    resume_session: Option<ResumeSession>,
    hibernating: bool,
}

/// Result of `almanac run`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RunReport {
    /// A real Windows process Astra spawned and now tracks.
    HostApp { process: PcbView },
    /// A host file / shortcut handed to Windows' default handler (a `.lnk`
    /// chains to its own process, so this is deliberately *not* tracked).
    HostOpen { display: String, real: String },
    /// A Microsoft Store / MSIX app Astra already asked the Windows shell to
    /// launch. Windows owns the real process, so this is *not* tracked and
    /// there is nothing left for the UI to open.
    HostLaunched { display: String },
    /// A simulated built-in Astra app or game.
    Builtin {
        process: PcbView,
        window: Option<String>,
    },
}

#[derive(Clone)]
pub struct AppState(pub Arc<RwLock<SystemState>>);

impl SystemState {
    pub fn from_snapshot(mut snapshot: PersistentSnapshot, persistence: JsonPersistence) -> Self {
        let hibernate = snapshot.hibernate.take();
        let existing_ids = snapshot.filesystem.existing_ids();
        let mut filesystem = snapshot.filesystem;
        let mut security = SecurityManager::new(snapshot.security);
        security.retain_existing_resources(&existing_ids);
        let locked_ids = security.locked_resource_ids();
        for resource_id in &existing_ids {
            if let Ok(resource) = filesystem.resource_mut_by_id(*resource_id) {
                resource.metadata.locked = locked_ids.contains(resource_id);
            }
        }
        let host = HostFilesystem::restore(&snapshot.host_mounts, &HostDirs::default());
        let (processes, scheduler, memory, resume_session) = match hibernate {
            Some(runtime) => (
                ProcessManager::from_runtime_snapshot(runtime.processes),
                Scheduler::from_runtime_snapshot(runtime.scheduler),
                MemoryManager::from_runtime_snapshot(runtime.memory),
                Some(ResumeSession {
                    cwd: runtime.cwd,
                    ui_session: runtime.ui_session,
                    almanac_session: runtime.almanac_session,
                }),
            ),
            None => (
                ProcessManager::new(),
                Scheduler::new(),
                MemoryManager::new(),
                None,
            ),
        };
        let hibernating = resume_session.is_some();
        let state = Self {
            filesystem,
            security,
            settings: snapshot.settings,
            command_history: snapshot.command_history,
            persistence,
            host,
            processes,
            scheduler,
            memory,
            pending: PendingPrompt::None,
            prompt_counter: 0,
            resume_session,
            hibernating,
        };
        // Consuming a hibernate image is atomic and one-shot. A later ordinary
        // restart must never resurrect this runtime again.
        if state.resume_session.is_some() {
            let _ = state.persist();
        }
        state
    }

    pub fn fresh(persistence: JsonPersistence) -> Self {
        Self::from_snapshot(PersistentSnapshot::default(), persistence)
    }

    /// Re-derive the host mount table with the real Windows user directories
    /// (called once from `lib.rs` with Tauri's path resolver). Persisted user
    /// mounts are preserved; the standard Desktop/Documents/Downloads/Projects
    /// defaults are (re)installed where they exist.
    pub fn install_host_dirs(&mut self, dirs: HostDirs) {
        let user = self.host.user_records();
        self.host = HostFilesystem::restore(&user, &dirs);
    }

    pub fn authentication_status(&self) -> AuthenticationStatus {
        self.security.status()
    }

    pub fn require_authentication(&self) -> Result<(), AstraError> {
        self.security.require_login()
    }

    pub fn configure_login(&mut self, password: &str) -> Result<AuthenticationStatus, AstraError> {
        self.transact(|state| state.security.configure_login(password))
    }

    pub fn login(&mut self, password: &str) -> Result<AuthenticationStatus, AstraError> {
        self.security.login(password)
    }

    pub fn logout(&mut self) -> AuthenticationStatus {
        self.pending = PendingPrompt::None;
        self.security.logout();
        self.security.status()
    }

    /// Confirm a password matches the stored login hash without touching the
    /// failed-attempt counter (used to gate `almanac logout`).
    pub fn verify_login_password(&self, password: &str) -> bool {
        self.security.verify_login_password(password)
    }

    pub fn lifecycle_summary(&mut self) -> crate::process::LifecycleProcessSummary {
        self.processes.lifecycle_summary()
    }

    pub fn prepare_hibernate(
        &mut self,
        cwd: String,
        ui_session: serde_json::Value,
        almanac_session: serde_json::Value,
    ) -> Result<(), AstraError> {
        self.require_authentication()?;
        let mut snapshot = self.snapshot();
        snapshot.hibernate = Some(HibernateSnapshot {
            processes: self.processes.runtime_snapshot(),
            scheduler: self.scheduler.runtime_snapshot(),
            memory: self.memory.runtime_snapshot(),
            cwd,
            ui_session,
            almanac_session,
        });
        self.persistence
            .save(&snapshot)
            .map(|_| self.hibernating = true)
    }

    pub fn prepare_restart(&mut self) -> Result<(), AstraError> {
        self.require_authentication()?;
        self.processes.shutdown_managed();
        self.scheduler = Scheduler::new();
        self.memory = MemoryManager::new();
        self.persist()
    }

    pub fn prepare_shutdown(&mut self) -> Result<(), AstraError> {
        self.require_authentication()?;
        self.processes.shutdown_managed();
        self.scheduler = Scheduler::new();
        self.memory = MemoryManager::new();
        self.persist()
    }

    pub fn take_resume_session(&mut self) -> Option<ResumeSession> {
        self.resume_session.take()
    }

    pub fn resume_runtime(&mut self) -> Option<ResumeSession> {
        self.hibernating = false;
        self.take_resume_session()
    }

    pub fn has_resumed_runtime(&self) -> bool {
        self.resume_session.is_some()
    }

    // ------------------------------------------------------------------
    // Almanac interactive-prompt state (process-local, never persisted)
    // ------------------------------------------------------------------

    pub fn has_pending_prompt(&self) -> bool {
        !matches!(self.pending, PendingPrompt::None)
    }

    pub fn set_pending_prompt(&mut self, prompt: PendingPrompt) {
        self.pending = prompt;
    }

    pub fn take_pending_prompt(&mut self) -> PendingPrompt {
        std::mem::take(&mut self.pending)
    }

    pub fn next_prompt_id(&mut self) -> String {
        self.prompt_counter = self.prompt_counter.wrapping_add(1);
        format!("prompt-{}", self.prompt_counter)
    }

    // ------------------------------------------------------------------
    // Command history (persistent; passwords never reach it — they only
    // ever arrive through the prompt-response path)
    // ------------------------------------------------------------------

    pub fn record_command_history(&mut self, line: &str) {
        let line = line.trim();
        if line.is_empty() {
            return;
        }
        self.command_history.push(line.to_string());
        if self.command_history.len() > MAX_HISTORY_ENTRIES {
            let overflow = self.command_history.len() - MAX_HISTORY_ENTRIES;
            self.command_history.drain(0..overflow);
        }
        // Best-effort: history should survive restart, but a persistence hiccup
        // must not fail the command the user just ran.
        let _ = self.persist();
    }

    pub fn command_history(&self) -> &[String] {
        &self.command_history
    }

    // ------------------------------------------------------------------
    // Helpers used by the Almanac engine / completion
    // ------------------------------------------------------------------

    /// Canonical `ROOT>…` path for a resolvable path, requiring login.
    pub fn canonical_path(&self, cwd: &str, path: &str) -> Result<String, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        Ok(self.filesystem.resource_path(id))
    }

    /// Validate that a directory can be locked *before* prompting for a
    /// password: it must exist, be a directory, have every ancestor lock
    /// already cleared, and not already be locked.
    pub fn precheck_lock(&self, cwd: &str, path: &str) -> Result<String, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        if self.filesystem.resource_by_id(id)?.metadata.resource_type != ResourceType::Directory {
            return Err(AstraError::NotADirectory(self.filesystem.resource_path(id)));
        }
        self.require_ancestor_locks_excluding(id)?;
        if self.security.is_resource_locked(id) {
            return Err(AstraError::InvalidArgument(format!(
                "{} is already locked",
                self.filesystem.resource_path(id)
            )));
        }
        Ok(self.filesystem.resource_path(id))
    }

    /// Validate that a directory can be unlocked before prompting.
    pub fn precheck_unlock(&self, cwd: &str, path: &str) -> Result<String, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.require_ancestor_locks_excluding(id)?;
        if !self.security.is_resource_locked(id) {
            return Err(AstraError::InvalidArgument(format!(
                "{} is not locked",
                self.filesystem.resource_path(id)
            )));
        }
        Ok(self.filesystem.resource_path(id))
    }

    /// Directory children for tab completion.
    ///
    /// Returns `(name, is_directory)` pairs. Errors with
    /// [`AstraError::ResourceAuthenticationRequired`] when the target directory
    /// (or an ancestor) is behind an un-cleared lock boundary, so completion
    /// never leaks the contents of a locked tree.
    pub fn completion_children(
        &self,
        cwd: &str,
        relative: &str,
    ) -> Result<Vec<(String, bool)>, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, relative)?;
        self.require_lock_boundaries(id)?;
        if self.security.is_resource_locked(id) && self.security.require_boundaries(&[id]).is_err()
        {
            return Err(AstraError::ResourceAuthenticationRequired(
                self.filesystem.resource_path(id),
            ));
        }
        self.authorize_path_id(id, &[AccessRequirement::Read, AccessRequirement::Execute])?;
        Ok(self
            .filesystem
            .list_directory(cwd, relative)?
            .into_iter()
            .map(|info| {
                let is_dir = info.metadata.resource_type == ResourceType::Directory;
                (info.metadata.name, is_dir)
            })
            .collect())
    }

    pub fn root(&self) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        self.authorize_path_id(
            ROOT_ID,
            &[AccessRequirement::Read, AccessRequirement::Execute],
        )?;
        Ok(self.filesystem.root_directory())
    }

    pub fn resolve_path(&self, cwd: &str, path: &str) -> Result<ResourceId, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Execute])?;
        Ok(id)
    }

    pub fn open_directory(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        if self.filesystem.resource_by_id(id)?.metadata.resource_type != ResourceType::Directory {
            return Err(AstraError::NotADirectory(self.filesystem.resource_path(id)));
        }
        self.authorize_path_id(id, &[AccessRequirement::Read, AccessRequirement::Execute])?;
        self.filesystem.resource_info(id)
    }

    pub fn parent_directory(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        let parent_id = self
            .filesystem
            .resource_by_id(id)?
            .metadata
            .parent
            .unwrap_or(ROOT_ID);
        self.authorize_path_id(
            parent_id,
            &[AccessRequirement::Read, AccessRequirement::Execute],
        )?;
        self.filesystem.resource_info(parent_id)
    }

    pub fn list_directory(&self, cwd: &str, path: &str) -> Result<Vec<ResourceInfo>, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Read, AccessRequirement::Execute])?;
        self.filesystem.list_directory(cwd, path)
    }

    pub fn inspect(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Read])?;
        self.filesystem.resource_info(id)
    }

    pub fn create_directory(&mut self, cwd: &str, path: &str) -> Result<ResourceInfo, AstraError> {
        let parent_id = self.resolve_parent_for_creation(cwd, path)?;
        self.authorize_path_id(
            parent_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.transact(|state| state.filesystem.create_directory(cwd, path))
    }

    pub fn create_file(
        &mut self,
        cwd: &str,
        path: &str,
        content: &str,
    ) -> Result<ResourceInfo, AstraError> {
        let parent_id = self.resolve_parent_for_creation(cwd, path)?;
        self.authorize_path_id(
            parent_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.transact(|state| state.filesystem.create_file(cwd, path, content))
    }

    pub fn write_file(
        &mut self,
        cwd: &str,
        path: &str,
        content: &str,
    ) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Write])?;
        self.transact(|state| state.filesystem.write_file(cwd, path, content))
    }

    pub fn read_file(&self, cwd: &str, path: &str) -> Result<String, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Read])?;
        self.filesystem.read_file(cwd, path)
    }

    pub fn read_file_bytes(&self, cwd: &str, path: &str) -> Result<Vec<u8>, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Read])?;
        self.filesystem.read_file_bytes(cwd, path)
    }

    pub fn create_file_bytes(
        &mut self,
        cwd: &str,
        path: &str,
        data: &[u8],
    ) -> Result<ResourceInfo, AstraError> {
        let parent_id = self.resolve_parent_for_creation(cwd, path)?;
        self.authorize_path_id(
            parent_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.transact(|state| state.filesystem.create_file_bytes(cwd, path, data))
    }

    pub fn create_tree(&mut self, cwd: &str, expression: &str) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let parent_id = self.filesystem.resolve_path(cwd, ".")?;
        self.authorize_path_id(
            parent_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.transact(|state| state.filesystem.create_tree_atomic(cwd, expression))
    }

    pub fn rename(
        &mut self,
        cwd: &str,
        path: &str,
        new_name: &str,
    ) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Write])?;
        let parent_id = self.parent_id(id)?;
        self.require_permissions(parent_id, &[AccessRequirement::Write])?;
        self.transact(|state| state.filesystem.rename(cwd, path, new_name))
    }

    pub fn move_resource(
        &mut self,
        cwd: &str,
        source_path: &str,
        destination_directory: &str,
    ) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let source_id = self.filesystem.resolve_path(cwd, source_path)?;
        let destination_id = self.filesystem.resolve_path(cwd, destination_directory)?;
        self.authorize_subtree(source_id, AccessRequirement::Write)?;
        self.authorize_path_id(
            destination_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.require_permissions(self.parent_id(source_id)?, &[AccessRequirement::Write])?;
        self.transact(|state| {
            state
                .filesystem
                .move_resource(cwd, source_path, destination_directory)
        })
    }

    pub fn copy_resource(
        &mut self,
        cwd: &str,
        source_path: &str,
        destination_directory: &str,
    ) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let source_id = self.filesystem.resolve_path(cwd, source_path)?;
        let destination_id = self.filesystem.resolve_path(cwd, destination_directory)?;
        self.authorize_subtree(source_id, AccessRequirement::Read)?;
        self.authorize_path_id(
            destination_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;
        self.transact(|state| {
            let copied = state
                .filesystem
                .copy_resource(cwd, source_path, destination_directory)?;
            let pairs = state
                .filesystem
                .parallel_subtree_pairs(source_id, copied.metadata.id)?;
            state.security.copy_resource_locks(&pairs);
            Ok(copied)
        })
    }

    pub fn delete_preview(&self, cwd: &str, path: &str) -> Result<DeleteSummary, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_subtree(id, AccessRequirement::Read)?;
        self.filesystem.delete_preview(cwd, path)
    }

    pub fn delete_recursive(&mut self, cwd: &str, path: &str) -> Result<DeleteSummary, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_subtree(id, AccessRequirement::Write)?;
        self.require_permissions(self.parent_id(id)?, &[AccessRequirement::Write])?;
        self.transact(|state| {
            let result = state.filesystem.delete_recursive(cwd, path)?;
            let existing = state.filesystem.existing_ids();
            state.security.retain_existing_resources(&existing);
            Ok(result)
        })
    }

    pub fn search(
        &self,
        cwd: &str,
        start_path: &str,
        query: &str,
    ) -> Result<SearchResults, AstraError> {
        self.security.require_login()?;
        if query.is_empty() {
            return Err(AstraError::InvalidArgument(
                "search query cannot be empty".to_string(),
            ));
        }
        let start_id = self.filesystem.resolve_path(cwd, start_path)?;
        let mut results = SearchResults::default();

        let ancestors = self.filesystem.ancestor_ids(start_id)?;
        if let Err(boundary_id) = self.security.require_boundaries(&ancestors) {
            results
                .skipped_subtrees
                .push(self.filesystem.resource_path(boundary_id));
            return Ok(results);
        }
        for ancestor_id in ancestors.iter().take(ancestors.len().saturating_sub(1)) {
            if !self.permission_allowed(*ancestor_id, AccessRequirement::Execute)? {
                results
                    .skipped_subtrees
                    .push(self.filesystem.resource_path(*ancestor_id));
                return Ok(results);
            }
        }
        self.search_secure(start_id, query, &mut results)?;
        Ok(results)
    }

    pub fn set_permissions(
        &mut self,
        cwd: &str,
        path: &str,
        permissions: Permissions,
    ) -> Result<ResourceInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.require_lock_boundaries(id)?;
        self.transact(|state| {
            let resource = state.filesystem.resource_mut_by_id(id)?;
            resource.metadata.permissions = permissions;
            resource.metadata.modified_at_ms = crate::filesystem::model::unix_time_ms();
            state.filesystem.resource_info(id)
        })
    }

    pub fn lock_resource(
        &mut self,
        cwd: &str,
        path: &str,
        password: &str,
    ) -> Result<ResourceSecurityInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        if self.filesystem.resource_by_id(id)?.metadata.resource_type != ResourceType::Directory {
            return Err(AstraError::NotADirectory(self.filesystem.resource_path(id)));
        }
        self.require_ancestor_locks_excluding(id)?;
        self.transact(|state| {
            state.security.add_resource_lock(id, password)?;
            let resource = state.filesystem.resource_mut_by_id(id)?;
            resource.metadata.locked = true;
            resource.metadata.modified_at_ms = crate::filesystem::model::unix_time_ms();
            state.security_info_for_id(id)
        })
    }

    pub fn authenticate_resource(
        &mut self,
        cwd: &str,
        path: &str,
        password: &str,
    ) -> Result<ResourceAuthenticationStatus, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        let boundaries = self.filesystem.ancestor_ids(id)?;
        let authenticated = self
            .security
            .authenticate_next_boundary(&boundaries, password)?;
        let remaining = boundaries
            .iter()
            .filter(|boundary| {
                self.security.is_resource_locked(**boundary)
                    && self.security.require_boundaries(&[**boundary]).is_err()
            })
            .count();
        Ok(ResourceAuthenticationStatus {
            path: self.filesystem.resource_path(id),
            authenticated_boundary_id: authenticated,
            remaining_boundaries: remaining,
        })
    }

    pub fn unlock_resource(
        &mut self,
        cwd: &str,
        path: &str,
        password: &str,
    ) -> Result<ResourceSecurityInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.require_ancestor_locks_excluding(id)?;
        self.transact(|state| {
            state.security.remove_resource_lock(id, password)?;
            let resource = state.filesystem.resource_mut_by_id(id)?;
            resource.metadata.locked = false;
            resource.metadata.modified_at_ms = crate::filesystem::model::unix_time_ms();
            state.security_info_for_id(id)
        })
    }

    pub fn resource_security_info(
        &self,
        cwd: &str,
        path: &str,
    ) -> Result<ResourceSecurityInfo, AstraError> {
        self.security.require_login()?;
        let id = self.filesystem.resolve_path(cwd, path)?;
        self.authorize_path_id(id, &[AccessRequirement::Read])?;
        self.security_info_for_id(id)
    }

    pub fn persistence_path(&self) -> &std::path::Path {
        self.persistence.path()
    }

    fn resolve_parent_for_creation(&self, cwd: &str, path: &str) -> Result<ResourceId, AstraError> {
        self.security.require_login()?;
        let canonical = normalize_path(cwd, path)?;
        let (parent_path, _) = split_path(&canonical)?;
        self.filesystem.resolve_path("ROOT", &parent_path)
    }

    fn parent_id(&self, id: ResourceId) -> Result<ResourceId, AstraError> {
        self.filesystem
            .resource_by_id(id)?
            .metadata
            .parent
            .ok_or_else(|| AstraError::PermissionDenied("ROOT cannot be modified".to_string()))
    }

    fn authorize_path_id(
        &self,
        id: ResourceId,
        target_requirements: &[AccessRequirement],
    ) -> Result<(), AstraError> {
        self.require_lock_boundaries(id)?;
        let ancestors = self.filesystem.ancestor_ids(id)?;
        for ancestor_id in ancestors.iter().take(ancestors.len().saturating_sub(1)) {
            self.require_permissions(*ancestor_id, &[AccessRequirement::Execute])?;
        }
        self.require_permissions(id, target_requirements)
    }

    fn authorize_subtree(
        &self,
        id: ResourceId,
        requirement: AccessRequirement,
    ) -> Result<(), AstraError> {
        self.authorize_path_id(id, &[requirement])?;
        for child_id in self.filesystem.subtree_ids(id)? {
            self.require_lock_boundaries(child_id)?;
            self.require_permissions(child_id, &[requirement])?;
            if self
                .filesystem
                .resource_by_id(child_id)?
                .metadata
                .resource_type
                == ResourceType::Directory
            {
                self.require_permissions(child_id, &[AccessRequirement::Execute])?;
            }
        }
        Ok(())
    }

    fn require_lock_boundaries(&self, id: ResourceId) -> Result<(), AstraError> {
        let boundaries = self.filesystem.ancestor_ids(id)?;
        self.security
            .require_boundaries(&boundaries)
            .map_err(|boundary_id| {
                AstraError::ResourceAuthenticationRequired(
                    self.filesystem.resource_path(boundary_id),
                )
            })
    }

    fn require_ancestor_locks_excluding(&self, id: ResourceId) -> Result<(), AstraError> {
        let mut boundaries = self.filesystem.ancestor_ids(id)?;
        boundaries.pop();
        self.security
            .require_boundaries(&boundaries)
            .map_err(|boundary_id| {
                AstraError::ResourceAuthenticationRequired(
                    self.filesystem.resource_path(boundary_id),
                )
            })
    }

    fn require_permissions(
        &self,
        id: ResourceId,
        requirements: &[AccessRequirement],
    ) -> Result<(), AstraError> {
        for requirement in requirements {
            if !self.permission_allowed(id, *requirement)? {
                let permission = match requirement {
                    AccessRequirement::Read => "READ",
                    AccessRequirement::Write => "WRITE",
                    AccessRequirement::Execute => "EXECUTE",
                };
                return Err(AstraError::PermissionDenied(format!(
                    "{permission} is disabled for {}",
                    self.filesystem.resource_path(id)
                )));
            }
        }
        Ok(())
    }

    fn permission_allowed(
        &self,
        id: ResourceId,
        requirement: AccessRequirement,
    ) -> Result<bool, AstraError> {
        let permissions = &self.filesystem.resource_by_id(id)?.metadata.permissions;
        Ok(match requirement {
            AccessRequirement::Read => permissions.read,
            AccessRequirement::Write => permissions.write,
            AccessRequirement::Execute => permissions.execute,
        })
    }

    fn search_secure(
        &self,
        id: ResourceId,
        query: &str,
        results: &mut SearchResults,
    ) -> Result<(), AstraError> {
        if self.security.is_resource_locked(id) && self.security.require_boundaries(&[id]).is_err()
        {
            results
                .skipped_subtrees
                .push(self.filesystem.resource_path(id));
            return Ok(());
        }
        let resource = self.filesystem.resource_by_id(id)?;
        if !resource.metadata.permissions.read
            || (resource.metadata.resource_type == ResourceType::Directory
                && !resource.metadata.permissions.execute)
        {
            results
                .skipped_subtrees
                .push(self.filesystem.resource_path(id));
            return Ok(());
        }
        if resource.metadata.name.contains(query) {
            results.matches.push(self.filesystem.resource_info(id)?);
        }
        if let Some(children) = resource.children() {
            for child_id in children.values() {
                self.search_secure(*child_id, query, results)?;
            }
        }
        Ok(())
    }

    fn security_info_for_id(&self, id: ResourceId) -> Result<ResourceSecurityInfo, AstraError> {
        let pending_lock_boundaries = self
            .filesystem
            .ancestor_ids(id)?
            .into_iter()
            .filter(|boundary_id| {
                self.security.is_resource_locked(*boundary_id)
                    && self.security.require_boundaries(&[*boundary_id]).is_err()
            })
            .map(|boundary_id| self.filesystem.resource_path(boundary_id))
            .collect();
        Ok(ResourceSecurityInfo {
            resource: self.filesystem.resource_info(id)?,
            pending_lock_boundaries,
        })
    }

    // ==================================================================
    // Phase 4 — host filesystem bridge
    //
    // Every host operation is routed here in Rust. React only ever sends the
    // `HOST>alias>rel…` scheme; it never sees or supplies a raw Windows path
    // (the sole exception is a directory the user explicitly picked in the
    // native mount dialog, which is canonicalised and containment-checked
    // before it becomes a mount).
    // ==================================================================

    /// Decide which provider a `cwd` + `path` pair addresses. Routing logic
    /// lives only here, never in React.
    pub fn route(&self, cwd: &str, path: &str) -> Result<AstraLocation, AstraError> {
        self.security.require_login()?;
        route(cwd, path)
    }

    pub fn host_mount_list(&self) -> Result<Vec<MountView>, AstraError> {
        self.security.require_login()?;
        Ok(self.host.list_mounts())
    }

    pub fn host_mount(
        &mut self,
        source: &Path,
        requested_alias: Option<&str>,
    ) -> Result<String, AstraError> {
        self.security.require_login()?;
        self.transact(|state| state.host.mount(source, requested_alias))
    }

    pub fn host_unmount(&mut self, alias: &str) -> Result<(), AstraError> {
        self.security.require_login()?;
        self.transact(|state| state.host.unmount(alias))
    }

    /// Ancestor host-lock boundary check → friendly error path.
    fn host_require_boundaries(&self, alias: &str, relative: &[String]) -> Result<(), AstraError> {
        let ancestor_ids = self.host.ancestor_ids(alias, relative)?;
        self.security
            .require_host_boundaries(&ancestor_ids)
            .map_err(|blocking_id| {
                AstraError::ResourceAuthenticationRequired(self.host.display_for_id(&blocking_id))
            })
    }

    fn host_view(
        &self,
        alias: &str,
        relative: &[String],
        entry: HostEntry,
    ) -> Result<EntryView, AstraError> {
        let display_path = if relative.is_empty() {
            format!("HOST>{alias}")
        } else {
            format!("HOST>{alias}>{}", relative.join(">"))
        };
        let canonical_id = self.host.canonical_id(alias, relative).unwrap_or_default();
        let ancestor_ids = self.host.ancestor_ids(alias, relative).unwrap_or_default();
        let astra_locked = self.security.is_host_locked(&canonical_id)
            || ancestor_ids
                .iter()
                .any(|id| self.security.is_host_locked(id));
        Ok(EntryView {
            display_path,
            name: entry.name,
            kind: ProviderKind::Host,
            is_dir: entry.is_dir,
            size: entry.size,
            modified_ms: entry.modified_ms,
            created_ms: entry.created_ms,
            read_only: entry.read_only,
            astra_locked,
            host_real_path: self
                .host
                .real_path(alias, relative)
                .ok()
                .map(|path| path.to_string_lossy().to_string()),
        })
    }

    pub fn host_open(&self, alias: &str, relative: &[String]) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self.host.entry(alias, relative)?;
        if !entry.is_dir {
            return Err(AstraError::NotADirectory(format!(
                "HOST>{alias}>{}",
                relative.join(">")
            )));
        }
        self.host_view(alias, relative, entry)
    }

    pub fn host_list(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<Vec<EntryView>, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let mut views = Vec::new();
        for entry in self.host.list_dir(alias, relative)? {
            let mut child = relative.to_vec();
            child.push(entry.name.clone());
            views.push(self.host_view(alias, &child, entry)?);
        }
        Ok(views)
    }

    pub fn host_inspect(&self, alias: &str, relative: &[String]) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self.host.entry(alias, relative)?;
        self.host_view(alias, relative, entry)
    }

    pub fn host_read(&self, alias: &str, relative: &[String]) -> Result<String, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        self.host.read_text(alias, relative)
    }

    pub fn host_read_bytes(&self, alias: &str, relative: &[String]) -> Result<Vec<u8>, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        self.host.read_bytes(alias, relative)
    }

    pub fn host_write(
        &mut self,
        alias: &str,
        relative: &[String],
        contents: &str,
        must_exist: bool,
    ) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self
            .host
            .write_text(alias, relative, contents, must_exist)?;
        self.host_view(alias, relative, entry)
    }

    pub fn host_write_bytes(
        &mut self,
        alias: &str,
        relative: &[String],
        data: &[u8],
        must_exist: bool,
    ) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self.host.write_bytes(alias, relative, data, must_exist)?;
        self.host_view(alias, relative, entry)
    }

    // ---- cross-boundary bulk copy (HOST ↔ ASTRA) ----

    /// Recursively copy a host file/tree at `from_rel` into the virtual
    /// directory `dest_dir` (canonical `ROOT>…`). The whole walk runs inside a
    /// single transaction — one clone, one persist — instead of one persist per
    /// file, which is what made large imports hang.
    pub fn import_host_into_virtual(
        &mut self,
        from_alias: &str,
        from_rel: &[String],
        dest_dir: &str,
    ) -> Result<CrossCopySummary, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(from_alias, from_rel)?;
        let dest_id = self.filesystem.resolve_path("ROOT", dest_dir)?;
        self.authorize_path_id(
            dest_id,
            &[AccessRequirement::Write, AccessRequirement::Execute],
        )?;

        let alias = from_alias.to_string();
        let root_rel = from_rel.to_vec();
        let dest = dest_dir.to_string();
        self.transact(move |state| {
            let mut summary = CrossCopySummary::default();
            summary.created_path =
                state.import_host_node(&alias, &root_rel, &dest, &mut summary)?;
            Ok(summary)
        })
    }

    fn import_host_node(
        &mut self,
        alias: &str,
        rel: &[String],
        dest_dir: &str,
        summary: &mut CrossCopySummary,
    ) -> Result<String, AstraError> {
        self.host_require_boundaries(alias, rel)?;
        let entry = self.host.entry(alias, rel)?;
        let child = format!("{dest_dir}>{}", entry.name);
        if entry.is_dir {
            self.filesystem.create_directory("ROOT", &child)?;
            summary.dirs += 1;
            for item in self.host.list_dir(alias, rel)? {
                let mut sub = rel.to_vec();
                sub.push(item.name);
                if let Err(error) = self.import_host_node(alias, &sub, &child, summary) {
                    summary.note_skip(&host_scope(alias, &sub), &error);
                }
            }
        } else {
            if entry.size > MAX_CROSS_COPY_FILE_BYTES {
                return Err(AstraError::InvalidArgument(format!(
                    "{:.1} MiB — over the {} MiB per-file cross-copy limit",
                    entry.size as f64 / (1024.0 * 1024.0),
                    MAX_CROSS_COPY_FILE_BYTES / (1024 * 1024),
                )));
            }
            let bytes = self.host.read_bytes(alias, rel)?;
            match String::from_utf8(bytes) {
                Ok(text) => self.filesystem.create_file("ROOT", &child, &text)?,
                Err(not_utf8) => {
                    self.filesystem
                        .create_file_bytes("ROOT", &child, &not_utf8.into_bytes())?
                }
            };
            summary.files += 1;
        }
        Ok(child)
    }

    /// Recursively copy the virtual resource at `source` (canonical `ROOT>…`)
    /// into the host directory `to_rel` under `to_alias`. This only mutates the
    /// host filesystem, so it needs no Astra-state transaction.
    pub fn export_virtual_to_host(
        &mut self,
        source: &str,
        to_alias: &str,
        to_rel: &[String],
    ) -> Result<CrossCopySummary, AstraError> {
        self.security.require_login()?;
        let source_id = self.filesystem.resolve_path("ROOT", source)?;
        self.authorize_subtree(source_id, AccessRequirement::Read)?;
        self.host_require_boundaries(to_alias, to_rel)?;

        let mut summary = CrossCopySummary::default();
        summary.created_path = self.export_virtual_node(source, to_alias, to_rel, &mut summary)?;
        Ok(summary)
    }

    fn export_virtual_node(
        &self,
        source: &str,
        alias: &str,
        to_rel: &[String],
        summary: &mut CrossCopySummary,
    ) -> Result<String, AstraError> {
        let info = self.filesystem.inspect("ROOT", source)?;
        let mut child_rel = to_rel.to_vec();
        child_rel.push(info.metadata.name.clone());
        match info.metadata.resource_type {
            ResourceType::Directory => {
                self.host.create_dir(alias, &child_rel)?;
                summary.dirs += 1;
                for item in self.filesystem.list_directory("ROOT", source)? {
                    let entry_path = format!("{source}>{}", item.metadata.name);
                    if let Err(error) =
                        self.export_virtual_node(&entry_path, alias, &child_rel, summary)
                    {
                        summary.note_skip(&entry_path, &error);
                    }
                }
            }
            ResourceType::File => {
                let bytes = self.filesystem.read_file_bytes("ROOT", source)?;
                self.host.write_bytes(alias, &child_rel, &bytes, false)?;
                summary.files += 1;
            }
        }
        Ok(host_scope(alias, &child_rel))
    }

    pub fn host_create_dir(
        &mut self,
        alias: &str,
        relative: &[String],
    ) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self.host.create_dir(alias, relative)?;
        self.host_view(alias, relative, entry)
    }

    pub fn host_rename(
        &mut self,
        alias: &str,
        relative: &[String],
        new_name: &str,
    ) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        let entry = self.host.rename(alias, relative, new_name)?;
        let mut renamed = relative[..relative.len().saturating_sub(1)].to_vec();
        renamed.push(new_name.to_string());
        self.host_view(alias, &renamed, entry)
    }

    pub fn host_relocate(
        &mut self,
        from_alias: &str,
        from_relative: &[String],
        to_alias: &str,
        to_relative: &[String],
        copy: bool,
    ) -> Result<EntryView, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(from_alias, from_relative)?;
        self.host_require_boundaries(to_alias, to_relative)?;
        let entry = self
            .host
            .relocate(from_alias, from_relative, to_alias, to_relative, copy)?;
        let mut target = to_relative.to_vec();
        target.push(entry.name.clone());
        self.host_view(to_alias, &target, entry)
    }

    pub fn host_delete_preview(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<(u64, u64), AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        self.host.count_descendants(alias, relative)
    }

    pub fn host_recycle(
        &mut self,
        alias: &str,
        relative: &[String],
    ) -> Result<crate::fs_provider::host::HostDeleteOutcome, AstraError> {
        self.security.require_login()?;
        self.host_require_boundaries(alias, relative)?;
        self.host.recycle(alias, relative)
    }

    // ---- host Astra-level locks (metadata only; no ACL / encryption) ----

    pub fn host_precheck_lock(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<(String, String), AstraError> {
        self.security.require_login()?;
        // Every ancestor lock except the target itself must already be cleared.
        if relative.is_empty() {
            self.host_require_boundaries(alias, relative)?;
        } else {
            self.host_require_boundaries(alias, &relative[..relative.len() - 1])?;
        }
        let entry = self.host.entry(alias, relative)?;
        if !entry.is_dir {
            return Err(AstraError::NotADirectory(format!(
                "HOST>{alias}>{}",
                relative.join(">")
            )));
        }
        let id = self.host.canonical_id(alias, relative)?;
        if self.security.is_host_locked(&id) {
            return Err(AstraError::InvalidArgument(
                "this host directory is already locked".to_string(),
            ));
        }
        Ok((id, format!("HOST>{alias}>{}", relative.join(">"))))
    }

    pub fn host_precheck_unlock(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<(String, String), AstraError> {
        self.security.require_login()?;
        if !relative.is_empty() {
            self.host_require_boundaries(alias, &relative[..relative.len() - 1])?;
        }
        let id = self.host.canonical_id(alias, relative)?;
        if !self.security.is_host_locked(&id) {
            return Err(AstraError::InvalidArgument(
                "this host directory is not locked".to_string(),
            ));
        }
        Ok((id, format!("HOST>{alias}>{}", relative.join(">"))))
    }

    pub fn host_commit_lock(
        &mut self,
        canonical_id: &str,
        password: &str,
    ) -> Result<(), AstraError> {
        self.transact(|state| state.security.add_host_lock(canonical_id, password))
    }

    pub fn host_commit_unlock(
        &mut self,
        canonical_id: &str,
        password: &str,
    ) -> Result<(), AstraError> {
        self.transact(|state| state.security.remove_host_lock(canonical_id, password))
    }

    pub fn host_authenticate(
        &mut self,
        alias: &str,
        relative: &[String],
        password: &str,
    ) -> Result<ResourceAuthenticationStatus, AstraError> {
        self.security.require_login()?;
        let ancestor_ids = self.host.ancestor_ids(alias, relative)?;
        self.security
            .authenticate_host_boundary(&ancestor_ids, password)?;
        let remaining = ancestor_ids
            .iter()
            .filter(|id| {
                self.security.is_host_locked(id)
                    && self
                        .security
                        .require_host_boundaries(&[(*id).clone()])
                        .is_err()
            })
            .count();
        Ok(ResourceAuthenticationStatus {
            path: format!("HOST>{alias}>{}", relative.join(">")),
            authenticated_boundary_id: None,
            remaining_boundaries: remaining,
        })
    }

    /// `almanac lookout` — search the virtual filesystem *and* every mounted
    /// host root, tagging each hit with its origin.
    pub fn lookout(&self, query: &str) -> Result<LookoutResults, AstraError> {
        self.security.require_login()?;
        if query.trim().is_empty() {
            return Err(AstraError::InvalidArgument(
                "search query cannot be empty".to_string(),
            ));
        }
        let mut results = LookoutResults::default();
        let virtual_provider = VirtualFilesystemProvider::new(self);
        let host_provider = HostFilesystemProvider::new(&self.host, &self.security);
        crate::fs_provider::FilesystemProvider::search(&virtual_provider, query, &mut results.hits);
        crate::fs_provider::FilesystemProvider::search(&host_provider, query, &mut results.hits);
        if let Ok(secure) = self.search("ROOT", "ROOT", query) {
            results.skipped = secure.skipped_subtrees;
        }
        Ok(results)
    }

    pub fn host_mount_aliases(&self) -> Vec<String> {
        self.host.mount_aliases()
    }

    // ==================================================================
    // Phase 5 — application launcher & process manager
    // ==================================================================

    pub fn process_list(&mut self) -> Result<Vec<PcbView>, AstraError> {
        self.security.require_login()?;
        Ok(self.processes.list())
    }

    pub fn process_terminate(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        self.security.require_login()?;
        let view = self.processes.terminate(pid)?;
        self.scheduler.remove(pid);
        self.memory.release(pid);
        Ok(view)
    }

    pub fn process_suspend(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        self.security.require_login()?;
        let view = self.processes.suspend(pid)?;
        self.scheduler.suspend(pid);
        Ok(view)
    }

    pub fn process_resume(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        self.security.require_login()?;
        let view = self.processes.resume(pid)?;
        self.scheduler.resume(pid);
        Ok(view)
    }

    pub fn process_get(&self, pid: Pid) -> Option<PcbView> {
        self.processes.get(pid)
    }

    // ==================================================================
    // Phase 6 — virtual CPU scheduler
    // ==================================================================

    /// Advance the deterministic virtual-CPU simulation by `count` ticks and
    /// mirror every state transition it produces onto the process table. Not
    /// login-gated: it is an internal simulation step driven by the background
    /// scheduler thread; the user-facing `almanac scheduler tick` verb performs
    /// its own authentication check.
    pub fn scheduler_tick(&mut self, count: u64) {
        if self.hibernating {
            return;
        }
        for _ in 0..count.min(100_000) {
            for (pid, state) in self.scheduler.tick() {
                self.processes.set_state_from_scheduler(pid, state);
            }
            // A process on a core touches its working set — this is what drives
            // simulated page hits / faults deterministically.
            let running = self.scheduler.running_pids();
            self.memory.tick_access(&running);
        }
    }

    pub fn scheduler_snapshot(&self) -> Result<SchedulerSnapshot, AstraError> {
        self.security.require_login()?;
        Ok(self.scheduler.snapshot())
    }

    pub fn scheduler_set_algorithm(
        &mut self,
        algorithm: SchedulerAlgorithm,
    ) -> Result<SchedulerSnapshot, AstraError> {
        self.security.require_login()?;
        self.scheduler.set_algorithm(algorithm);
        Ok(self.scheduler.snapshot())
    }

    // ==================================================================
    // Phase 7 — simulated memory subsystem
    // ==================================================================

    pub fn memory_snapshot(&self) -> Result<MemorySnapshot, AstraError> {
        self.security.require_login()?;
        Ok(self.memory.snapshot())
    }

    pub fn memory_set_policy(
        &mut self,
        policy: ReplacementPolicy,
    ) -> Result<MemorySnapshot, AstraError> {
        self.security.require_login()?;
        self.memory.set_policy(policy);
        Ok(self.memory.snapshot())
    }

    /// Register (PID only) a streamed host-shell command in the process table.
    /// Called by the command layer once `SystemProcessRunner` reports the spawn.
    pub fn register_shell_process(&mut self, program: &str, display: &str, os_pid: u32) -> PcbView {
        let note = spawns_children_note(program);
        self.processes.register_host(
            program.to_string(),
            display.to_string(),
            ProcessType::HostCommand,
            self.processes.launcher_pid(),
            None,
            Some(os_pid),
            note,
        )
    }

    pub fn mark_shell_process_exited(&mut self, pid: Pid) {
        self.processes.mark_exited(pid);
    }

    /// `almanac run <application> [args]`.
    ///
    /// Resolution order:
    /// 1. a file / shortcut on a mounted host path
    ///    (`HOST>PublicDesktop>VALORANT.lnk`, or a bare name relative to a host
    ///    `cwd`),
    /// 2. a configured host-app alias / `PATH` / Windows `App Paths`,
    /// 3. the built-in Astra registry.
    pub fn run_application(
        &mut self,
        cwd: &str,
        application: &str,
        args: &[String],
    ) -> Result<RunReport, AstraError> {
        self.security.require_login()?;

        // 1. A real file/shortcut living on a mounted host directory.
        if let Ok(AstraLocation::Host { mount, relative }) = route(cwd, application) {
            if !relative.is_empty() {
                if let Ok(entry) = self.host.entry(&mount, &relative) {
                    if !entry.is_dir {
                        self.host_require_boundaries(&mount, &relative)?;
                        let real = self.host.real_path(&mount, &relative)?;
                        return self.launch_host_path(&entry.name, &real, args);
                    }
                }
            }
        }

        match resolve_host_app(application) {
            HostAppResolution::Found { display, program } => {
                let child = Command::new(&program)
                    .args(args)
                    .stdin(Stdio::null())
                    .spawn()
                    .map_err(|error| {
                        AstraError::HostProcess(format!("could not launch {display}: {error}"))
                    })?;
                let process = self.processes.register_host(
                    display.clone(),
                    program.display().to_string(),
                    ProcessType::HostApp,
                    self.processes.launcher_pid(),
                    Some(child),
                    None,
                    None,
                );
                Ok(RunReport::HostApp { process })
            }
            HostAppResolution::StoreApp { display, aumid } => {
                Command::new("explorer.exe")
                    .arg(format!("shell:AppsFolder\\{aumid}"))
                    .stdin(Stdio::null())
                    .spawn()
                    .map_err(|error| {
                        AstraError::HostProcess(format!("could not launch {display}: {error}"))
                    })?;
                Ok(RunReport::HostLaunched { display })
            }
            HostAppResolution::NotInstalled { alias } => Err(AstraError::CommandNotFound(format!(
                "{alias} is a registered application but is not installed (not on PATH or in \
                 Windows App Paths)"
            ))),
            HostAppResolution::Unknown => match find_builtin(application) {
                Some(def) => {
                    let pages = crate::memory::resident_pages_for(def.name);
                    let LaunchReport { process, window } = self
                        .processes
                        .spawn_builtin(def, self.processes.launcher_pid());
                    // Simulated Astra apps/games join the virtual CPU's READY
                    // queue; host processes never do.
                    if let Some(class) = ScheduleClass::from_process_type(process.process_type) {
                        self.scheduler.admit(
                            process.pid,
                            class,
                            process.priority,
                            Workload::for_class(class),
                        );
                        // Request simulated memory. On OOM, roll the launch
                        // back cleanly rather than leaving a process with no
                        // address space.
                        if let Err(error) = self.memory.allocate(process.pid, pages) {
                            self.scheduler.remove(process.pid);
                            let _ = self.processes.terminate(process.pid);
                            return Err(error);
                        }
                    }
                    Ok(RunReport::Builtin { process, window })
                }
                None => Err(AstraError::Process(format!(
                    "unknown application '{application}' — not a known host application and not a \
                     built-in Astra app"
                ))),
            },
        }
    }

    /// Launch a real file that lives on a mounted host path. `.exe` targets are
    /// spawned and tracked as `HOST_APP`; `.lnk` / `.url` / documents are given
    /// to Windows' default handler and left untracked.
    fn launch_host_path(
        &mut self,
        name: &str,
        real: &Path,
        args: &[String],
    ) -> Result<RunReport, AstraError> {
        let ext = real
            .extension()
            .and_then(|value| value.to_str())
            .unwrap_or("")
            .to_ascii_lowercase();

        if ext == "exe" {
            let child = Command::new(real)
                .args(args)
                .stdin(Stdio::null())
                .spawn()
                .map_err(|error| {
                    AstraError::HostProcess(format!("could not launch {name}: {error}"))
                })?;
            let process = self.processes.register_host(
                name.to_string(),
                real.display().to_string(),
                ProcessType::HostApp,
                self.processes.launcher_pid(),
                Some(child),
                None,
                None,
            );
            Ok(RunReport::HostApp { process })
        } else {
            Ok(RunReport::HostOpen {
                display: name.to_string(),
                real: real.to_string_lossy().to_string(),
            })
        }
    }

    /// Open a resolved real host file in a named application (used by
    /// `almanac write/rewrite … in <App>` for HOST resources).
    pub fn open_host_file_in_app(
        &mut self,
        application: &str,
        real_path: &str,
    ) -> Result<PcbView, AstraError> {
        self.security.require_login()?;
        match resolve_host_app(application) {
            HostAppResolution::Found { display, program } => {
                let child = Command::new(&program)
                    .arg(real_path)
                    .stdin(Stdio::null())
                    .spawn()
                    .map_err(|error| {
                        AstraError::HostProcess(format!("could not launch {display}: {error}"))
                    })?;
                Ok(self.processes.register_host(
                    display.clone(),
                    format!("{} {real_path}", program.display()),
                    ProcessType::HostApp,
                    self.processes.launcher_pid(),
                    Some(child),
                    None,
                    None,
                ))
            }
            HostAppResolution::StoreApp { display, .. } => Err(AstraError::Process(format!(
                "{display} is a Microsoft Store app and cannot be handed a file path directly"
            ))),
            HostAppResolution::NotInstalled { alias } => Err(AstraError::CommandNotFound(format!(
                "{alias} is not installed"
            ))),
            HostAppResolution::Unknown => Err(AstraError::Process(format!(
                "'{application}' is not a known application"
            ))),
        }
    }

    fn snapshot(&self) -> PersistentSnapshot {
        PersistentSnapshot {
            schema_version: CURRENT_SCHEMA_VERSION,
            filesystem: self.filesystem.clone(),
            security: self.security.persistent().clone(),
            settings: self.settings.clone(),
            command_history: self.command_history.clone(),
            host_mounts: self.host.user_records(),
            hibernate: None,
        }
    }

    fn persist(&self) -> Result<(), AstraError> {
        self.persistence.save(&self.snapshot())
    }

    fn transact<T>(
        &mut self,
        operation: impl FnOnce(&mut Self) -> Result<T, AstraError>,
    ) -> Result<T, AstraError> {
        let previous_filesystem = self.filesystem.clone();
        let previous_security = self.security.clone();
        let previous_settings = self.settings.clone();
        let previous_history = self.command_history.clone();
        let previous_host = self.host.clone();

        let restore = |state: &mut Self| {
            state.filesystem = previous_filesystem;
            state.security = previous_security;
            state.settings = previous_settings;
            state.command_history = previous_history;
            state.host = previous_host;
        };

        match operation(self) {
            Ok(value) => match self.persist() {
                Ok(()) => Ok(value),
                Err(error) => {
                    restore(self);
                    Err(error)
                }
            },
            Err(error) => {
                restore(self);
                Err(error)
            }
        }
    }
}

/// Some host launchers (`npm`, `python`, …) spawn their own child processes
/// that Windows — not Astra — manages. We annotate the PCB rather than
/// fabricate child entries.
fn spawns_children_note(program: &str) -> Option<String> {
    const LAUNCHERS: &[&str] = &[
        "npm", "npx", "yarn", "pnpm", "node", "pip", "pip3", "python", "python3", "cargo", "deno",
        "bun",
    ];
    let base = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .trim_end_matches(".exe")
        .trim_end_matches(".cmd")
        .to_ascii_lowercase();
    LAUNCHERS.contains(&base.as_str()).then(|| {
        "may spawn child processes that Windows manages and Astra does not track individually"
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_state(directory: &tempfile::TempDir) -> SystemState {
        SystemState::fresh(JsonPersistence::new(directory.path().join("state.json")))
    }

    #[test]
    fn locked_ancestors_and_nested_locks_require_each_password() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        state
            .create_tree("ROOT>Projects", "Vault>(Secret)")
            .unwrap();
        state
            .create_file("ROOT>Projects>Vault>Secret", "note.txt", "classified")
            .unwrap();
        state
            .lock_resource("ROOT", "Projects>Vault", "vault-password")
            .unwrap();

        assert!(matches!(
            state.open_directory("ROOT", "Projects>Vault>Secret"),
            Err(AstraError::ResourceAuthenticationRequired(path)) if path == "ROOT>Projects>Vault"
        ));
        let first = state
            .authenticate_resource("ROOT", "Projects>Vault>Secret", "vault-password")
            .unwrap();
        assert_eq!(first.remaining_boundaries, 0);
        state
            .lock_resource("ROOT", "Projects>Vault>Secret", "secret-password")
            .unwrap();
        state.logout();
        state.login("login-password").unwrap();

        assert!(state
            .write_file("ROOT", "Projects>Vault>Secret>note.txt", "changed")
            .is_err());
        let parent = state
            .authenticate_resource("ROOT", "Projects>Vault>Secret", "vault-password")
            .unwrap();
        assert_eq!(parent.remaining_boundaries, 1);
        assert!(matches!(
            state.open_directory("ROOT", "Projects>Vault>Secret"),
            Err(AstraError::ResourceAuthenticationRequired(path)) if path == "ROOT>Projects>Vault>Secret"
        ));
        let nested = state
            .authenticate_resource("ROOT", "Projects>Vault>Secret", "secret-password")
            .unwrap();
        assert_eq!(nested.remaining_boundaries, 0);
        assert!(state
            .open_directory("ROOT", "Projects>Vault>Secret")
            .is_ok());
    }

    #[test]
    fn permissions_are_independent_and_enforced() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        let updated = state
            .set_permissions(
                "ROOT",
                "Documents",
                Permissions {
                    read: false,
                    write: true,
                    execute: true,
                },
            )
            .unwrap();
        assert!(matches!(
            state.inspect("ROOT", "Documents"),
            Err(AstraError::PermissionDenied(_))
        ));
        assert!(!updated.metadata.locked);
        assert!(!updated.metadata.permissions.read);
    }

    #[test]
    fn search_skips_unauthenticated_locked_subtrees() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        state
            .create_tree("ROOT>Projects", "Public>(Needle)")
            .unwrap();
        state
            .create_tree("ROOT>Projects", "Private>(NeedleSecret)")
            .unwrap();
        state
            .lock_resource("ROOT", "Projects>Private", "private-password")
            .unwrap();

        let results = state.search("ROOT", "Projects", "Needle").unwrap();
        assert_eq!(results.matches.len(), 1);
        assert_eq!(results.skipped_subtrees, vec!["ROOT>Projects>Private"]);
    }

    #[test]
    fn persistent_state_reloads_without_restoring_the_session() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let mut state = SystemState::fresh(store.clone());
        state.configure_login("login-password").unwrap();
        state
            .create_file("ROOT>Projects", "saved.txt", "survives restart")
            .unwrap();
        state
            .lock_resource("ROOT", "Projects", "project-password")
            .unwrap();

        let persisted_text = std::fs::read_to_string(store.path()).unwrap();
        assert!(!persisted_text.contains("login-password"));
        assert!(!persisted_text.contains("project-password"));
        assert!(persisted_text.contains("$argon2"));

        let snapshot = store.load().unwrap().unwrap();
        let mut reloaded = SystemState::from_snapshot(snapshot, store);
        assert!(reloaded.authentication_status().configured);
        assert!(!reloaded.authentication_status().authenticated);
        reloaded.login("login-password").unwrap();
        assert!(matches!(
            reloaded.read_file("ROOT", "Projects>saved.txt"),
            Err(AstraError::ResourceAuthenticationRequired(_))
        ));
        reloaded
            .authenticate_resource("ROOT", "Projects", "project-password")
            .unwrap();
        assert_eq!(
            reloaded.read_file("ROOT", "Projects>saved.txt").unwrap(),
            "survives restart"
        );
    }

    #[test]
    fn copying_protected_contents_preserves_the_lock() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        state
            .create_tree("ROOT>Projects", "Vault>(Protected)")
            .unwrap();
        state
            .lock_resource("ROOT", "Projects>Vault>Protected", "secret-password")
            .unwrap();
        state
            .authenticate_resource("ROOT", "Projects>Vault>Protected", "secret-password")
            .unwrap();
        state
            .copy_resource("ROOT", "Projects>Vault", "Documents")
            .unwrap();

        assert!(matches!(
            state.open_directory("ROOT", "Documents>Vault>Protected"),
            Err(AstraError::ResourceAuthenticationRequired(_))
        ));
        state
            .authenticate_resource("ROOT", "Documents>Vault>Protected", "secret-password")
            .unwrap();
        assert!(state
            .open_directory("ROOT", "Documents>Vault>Protected")
            .is_ok());
    }

    #[test]
    fn user_host_mounts_and_locks_survive_reload_but_defaults_are_not_persisted() {
        let directory = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        std::fs::create_dir(work.path().join("Sealed")).unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));

        let mut state = SystemState::fresh(store.clone());
        state.configure_login("login-password").unwrap();
        let alias = state.host_mount(work.path(), Some("Dev")).unwrap();
        let (id, _display) = state
            .host_precheck_lock(&alias, &["Sealed".to_string()])
            .unwrap();
        state.host_commit_lock(&id, "vault-password").unwrap();

        let persisted = std::fs::read_to_string(store.path()).unwrap();
        assert!(persisted.contains("\"Dev\""));
        assert!(!persisted.contains("vault-password"));
        assert!(persisted.contains("$argon2"));

        let snapshot = store.load().unwrap().unwrap();
        assert_eq!(snapshot.host_mounts.len(), 1);
        let mut reloaded = SystemState::from_snapshot(snapshot, store);
        reloaded.login("login-password").unwrap();

        // Mount is back.
        let mounts = reloaded.host_mount_list().unwrap();
        assert!(mounts.iter().any(|m| m.alias == "Dev" && !m.is_default));
        // Lock is back — access is gated until the password is supplied.
        assert!(matches!(
            reloaded.host_open("Dev", &["Sealed".to_string()]),
            Err(AstraError::ResourceAuthenticationRequired(_))
        ));
    }

    #[test]
    fn run_application_launches_and_tracks_a_real_host_process() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();

        let (app, args): (&str, Vec<String>) = if cfg!(windows) {
            ("cmd", vec!["/C".into(), "pause".into()])
        } else {
            ("sh", vec!["-c".into(), "sleep 30".into()])
        };
        let report = state.run_application("ROOT", app, &args).unwrap();
        let RunReport::HostApp { process } = report else {
            panic!("a PATH command must launch as a host app");
        };
        assert!(process.host_backed);
        assert!(process.host_pid.is_some());
        assert!(!process.simulated);
        assert_eq!(process.parent_pid, Some(2)); // parented to the Almanac launcher

        // Only a PID Astra tracks can be terminated; unknown PIDs are rejected.
        assert!(state.process_terminate(987654).is_err());
        let killed = state.process_terminate(process.pid).unwrap();
        assert_eq!(format!("{:?}", killed.state), "Terminated");
    }

    #[test]
    fn builtin_registry_covers_every_required_app_and_game() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        for name in [
            "Almanac",
            "Terminal",
            "TaskManager",
            "Settings",
            "Calculator",
            "TextEditor",
            "ImageViewer",
            "Snake",
            "Pong",
            "Minesweeper",
            "Tetris",
        ] {
            assert!(
                matches!(
                    state.run_application("ROOT", name, &[]),
                    Ok(RunReport::Builtin { .. })
                ),
                "run {name} should register a built-in process"
            );
        }
    }

    #[test]
    fn launching_a_builtin_allocates_simulated_memory_and_terminating_frees_it() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();

        let base = state.memory_snapshot().unwrap().frames_used;
        let RunReport::Builtin { process, .. } =
            state.run_application("ROOT", "Calculator", &[]).unwrap()
        else {
            panic!("Calculator is a built-in");
        };
        // Calculator's centrally-defined footprint is 16 MB = 4 frames.
        let after = state.memory_snapshot().unwrap();
        assert_eq!(after.frames_used, base + 4);
        assert!(after
            .processes
            .iter()
            .any(|p| p.pid == process.pid && p.pages == 4));

        state.process_terminate(process.pid).unwrap();
        assert_eq!(state.memory_snapshot().unwrap().frames_used, base);
    }

    #[test]
    fn running_out_of_simulated_memory_fails_the_launch_without_a_zombie() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();

        // Calculator is the smallest footprint (4 pages); RAM + swap hold 2048
        // pages, so this eventually cannot be satisfied.
        let mut launched = 0u32;
        loop {
            match state.run_application("ROOT", "Calculator", &[]) {
                Ok(_) => launched += 1,
                Err(AstraError::OutOfMemory { .. }) => break,
                Err(other) => panic!("unexpected launch error: {other:?}"),
            }
            assert!(launched < 5000, "simulated memory never ran out");
        }

        // The rolled-back launch left no live process behind.
        let live_astra_apps = state
            .process_list()
            .unwrap()
            .into_iter()
            .filter(|p| {
                p.process_type == ProcessType::AstraApp
                    && p.state != crate::process::ProcessState::Terminated
            })
            .count();
        assert_eq!(live_astra_apps as u32, launched);
        // And the scheduler is not tracking a process with no address space.
        assert_eq!(
            state.scheduler_snapshot().unwrap().schedulable_count as u32,
            launched
        );
    }

    #[test]
    fn host_mount_rejects_a_path_that_is_not_a_directory() {
        let directory = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let file = work.path().join("a.txt");
        std::fs::write(&file, b"x").unwrap();
        let mut state = test_state(&directory);
        state.configure_login("login-password").unwrap();
        assert!(state.host_mount(&file, Some("Bad")).is_err());
        assert!(state
            .host_mount(&work.path().join("does-not-exist"), None)
            .is_err());
    }

    #[test]
    fn restart_preserves_durable_state_and_clears_runtime() {
        let directory = tempfile::tempdir().unwrap();
        let mut state = test_state(&directory);
        state.configure_login("correct-horse").unwrap();
        state
            .create_file("ROOT", "Documents>restart.txt", "durable")
            .unwrap();
        state.run_application("ROOT", "Calculator", &[]).unwrap();
        state.scheduler_tick(8);
        assert!(state.memory_snapshot().unwrap().ram_used_mb > 0);

        state.prepare_restart().unwrap();
        assert_eq!(state.memory_snapshot().unwrap().ram_used_mb, 0);
        assert_eq!(state.scheduler_snapshot().unwrap().schedulable_count, 0);

        let store = JsonPersistence::new(directory.path().join("state.json"));
        let snapshot = store.load().unwrap().unwrap();
        assert!(snapshot.hibernate.is_none());
        assert_eq!(
            snapshot
                .filesystem
                .read_file("ROOT", "Documents>restart.txt")
                .unwrap(),
            "durable"
        );
    }

    #[test]
    fn hibernate_snapshot_restores_runtime_and_ui_once() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let mut state = SystemState::fresh(store.clone());
        state.configure_login("correct-horse").unwrap();
        state.run_application("ROOT", "Calculator", &[]).unwrap();
        state.scheduler_tick(11);
        let before_tick = state.scheduler_snapshot().unwrap().tick;
        let before_ram = state.memory_snapshot().unwrap().ram_used_mb;
        state
            .prepare_hibernate(
                "ROOT>Projects".into(),
                serde_json::json!({"windows":[{"id":"calculator"}]}),
                serde_json::json!({"history":["almanac memory"]}),
            )
            .unwrap();

        let snapshot = store.load().unwrap().unwrap();
        assert!(snapshot.hibernate.is_some());
        let mut restored = SystemState::from_snapshot(snapshot, store.clone());
        assert!(!restored.authentication_status().authenticated);
        restored.login("correct-horse").unwrap();
        assert_eq!(restored.scheduler_snapshot().unwrap().tick, before_tick);
        assert_eq!(restored.memory_snapshot().unwrap().ram_used_mb, before_ram);
        restored.scheduler_tick(5);
        assert_eq!(restored.scheduler_snapshot().unwrap().tick, before_tick);
        let resume = restored.take_resume_session().unwrap();
        assert_eq!(resume.cwd, "ROOT>Projects");
        assert!(restored.take_resume_session().is_none());
        restored.resume_runtime();
        restored.scheduler_tick(1);
        assert_eq!(restored.scheduler_snapshot().unwrap().tick, before_tick + 1);
        assert!(store.load().unwrap().unwrap().hibernate.is_none());
    }

    #[test]
    fn disappeared_host_process_is_marked_terminated_after_resume() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let mut state = SystemState::fresh(store.clone());
        state.configure_login("correct-horse").unwrap();
        let host = state.register_shell_process("gone.exe", "gone.exe", u32::MAX - 1);
        state
            .prepare_hibernate(
                "ROOT".into(),
                serde_json::Value::Null,
                serde_json::Value::Null,
            )
            .unwrap();
        let mut restored = SystemState::from_snapshot(store.load().unwrap().unwrap(), store);
        restored.login("correct-horse").unwrap();
        let entry = restored
            .process_list()
            .unwrap()
            .into_iter()
            .find(|process| process.pid == host.pid)
            .unwrap();
        assert_eq!(entry.state, crate::process::ProcessState::Terminated);
        assert!(entry.note.unwrap().contains("no longer exists"));
    }
}
