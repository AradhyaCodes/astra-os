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
use std::io::{Read, Write};
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
        let mut notices = Vec::new();
        for source in [&self.path, &self.backup_path()] {
            if !source.exists() {
                continue;
            }
            match read_snapshot(source) {
                Ok(snapshot) => {
                    if source != &self.path {
                        notices.push("loaded the previous valid backup".to_string());
                    }
                    return Ok(LoadReport {
                        snapshot,
                        recovery_notice: (!notices.is_empty()).then(|| notices.join("; ")),
                    });
                }
                Err(error @ AstraError::CorruptPersistence(_))
                | Err(error @ AstraError::PersistenceTooLarge { .. }) => {
                    let kind = if matches!(error, AstraError::PersistenceTooLarge { .. }) {
                        "oversized"
                    } else {
                        "corrupt"
                    };
                    let quarantined = Self::quarantine_file(source, kind)?;
                    notices.push(format!("{error}; set aside as {}", quarantined.display()));
                }
                // An older app must not quarantine or overwrite a newer profile.
                Err(error) => return Err(error),
            }
        }
        Ok(LoadReport {
            snapshot: PersistentSnapshot::default(),
            recovery_notice: (!notices.is_empty()).then(|| notices.join("; ")),
        })
    }

    /// Import an older product profile atomically, without replacing an existing
    /// primary or recovery backup and without changing the source profile.
    pub fn migrate_legacy_profile(legacy: &Path, target: &Path) -> Result<bool, AstraError> {
        let target = Self::new(target.to_path_buf());
        if target.path.exists() || target.backup_path().exists() {
            return Ok(false);
        }
        let Some(snapshot) = Self::new(legacy.to_path_buf()).load()? else {
            return Ok(false);
        };
        target.save(&snapshot)?;
        Ok(true)
    }

    fn quarantine_file(source: &Path, kind: &str) -> Result<PathBuf, AstraError> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let filename = source.file_name().unwrap_or_default().to_string_lossy();
        let quarantined = source.with_file_name(format!("{filename}.{kind}-{timestamp}.json"));
        fs::rename(source, &quarantined).map_err(|error| {
            AstraError::Persistence(format!("could not quarantine state: {error}"))
        })?;
        Ok(quarantined)
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
        check_snapshot_size(bytes.len() as u64)?;
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
        if self.path.exists() {
            if backup.exists() {
                fs::remove_file(&backup).map_err(|error| {
                    AstraError::Persistence(format!("could not clear old state backup: {error}"))
                })?;
            }
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
        // Keep one previous committed snapshot for corruption/crash recovery.
        Ok(())
    }
}

fn check_snapshot_size(size: u64) -> Result<(), AstraError> {
    if size > MAX_STATE_FILE_BYTES {
        return Err(AstraError::PersistenceTooLarge {
            size,
            limit: MAX_STATE_FILE_BYTES,
        });
    }
    Ok(())
}

fn read_snapshot(path: &Path) -> Result<PersistentSnapshot, AstraError> {
    let file = fs::File::open(path)
        .map_err(|error| AstraError::Persistence(format!("could not open state: {error}")))?;
    let size = file
        .metadata()
        .map_err(|error| AstraError::Persistence(format!("could not inspect state: {error}")))?
        .len();
    check_snapshot_size(size)?;
    let mut bytes = Vec::new();
    file.take(MAX_STATE_FILE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| AstraError::Persistence(format!("could not read state: {error}")))?;
    check_snapshot_size(bytes.len() as u64)?;
    // Read the version before deserializing this app version's full schema.
    #[derive(Deserialize)]
    struct SchemaHeader {
        schema_version: u32,
    }
    let header: SchemaHeader = serde_json::from_slice(&bytes)
        .map_err(|_| AstraError::CorruptPersistence("invalid state JSON".to_string()))?;
    if !(MINIMUM_SCHEMA_VERSION..=CURRENT_SCHEMA_VERSION).contains(&header.schema_version) {
        return Err(AstraError::UnsupportedSchema(header.schema_version));
    }
    let mut snapshot: PersistentSnapshot = serde_json::from_slice(&bytes)
        .map_err(|_| AstraError::CorruptPersistence("invalid state JSON".to_string()))?;
    crate::filesystem::validation::validate_snapshot(&snapshot.filesystem)?;
    // Forward-migrate in memory; the next save rewrites at the current version.
    snapshot.schema_version = CURRENT_SCHEMA_VERSION;
    Ok(snapshot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn structurally_invalid_filesystems_are_rejected_before_runtime_use() {
        use crate::filesystem::model::{Resource, ROOT_ID};
        for case in [
            "missing root",
            "cycle",
            "missing child",
            "orphan",
            "counter",
            "size",
        ] {
            let directory = tempfile::tempdir().unwrap();
            let store = JsonPersistence::new(directory.path().join("state.json"));
            let mut snapshot = PersistentSnapshot::default();
            match case {
                "missing root" => {
                    snapshot.filesystem.resources.remove(&ROOT_ID);
                }
                "cycle" => {
                    snapshot
                        .filesystem
                        .resources
                        .get_mut(&ROOT_ID)
                        .unwrap()
                        .children_mut()
                        .unwrap()
                        .insert("ROOT".to_string(), ROOT_ID);
                }
                "missing child" => {
                    snapshot
                        .filesystem
                        .resources
                        .get_mut(&ROOT_ID)
                        .unwrap()
                        .children_mut()
                        .unwrap()
                        .insert("missing".to_string(), 9999);
                }
                "orphan" => {
                    let id = snapshot.filesystem.next_id;
                    snapshot.filesystem.next_id += 1;
                    snapshot.filesystem.resources.insert(
                        id,
                        Resource::directory(id, "orphan".to_string(), Some(ROOT_ID)),
                    );
                }
                "counter" => {
                    snapshot.filesystem.next_id = ROOT_ID;
                }
                "size" => {
                    let file = snapshot
                        .filesystem
                        .create_file("ROOT", "size.txt", "hello")
                        .unwrap();
                    snapshot
                        .filesystem
                        .resources
                        .get_mut(&file.metadata.id)
                        .unwrap()
                        .metadata
                        .size = 0;
                }
                _ => unreachable!(),
            }
            fs::write(store.path(), serde_json::to_vec(&snapshot).unwrap()).unwrap();
            assert!(
                matches!(store.load(), Err(AstraError::CorruptPersistence(_))),
                "{case}"
            );
        }
    }

    #[test]
    fn previous_commit_survives_primary_corruption_and_the_next_save() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let mut snapshot = PersistentSnapshot::default();
        snapshot.command_history.push("first".to_string());
        store.save(&snapshot).unwrap();
        snapshot.command_history.push("second".to_string());
        store.save(&snapshot).unwrap();
        assert!(store.backup_path().exists());
        fs::write(store.path(), b"broken").unwrap();
        let recovered = store.load_recovering().unwrap();
        assert_eq!(recovered.snapshot.command_history, ["first"]);
        assert!(recovered.recovery_notice.unwrap().contains("backup"));
        store.save(&recovered.snapshot).unwrap();
        assert!(store.backup_path().exists());
        assert_eq!(store.load().unwrap().unwrap().command_history, ["first"]);
    }

    #[test]
    fn oversized_backup_is_bounded_and_quarantined() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        File::create(store.backup_path())
            .unwrap()
            .set_len(MAX_STATE_FILE_BYTES + 1)
            .unwrap();
        assert!(matches!(
            store.load(),
            Err(AstraError::PersistenceTooLarge { .. })
        ));
        let report = store.load_recovering().unwrap();
        assert!(report.recovery_notice.unwrap().contains("over the"));
        assert!(!store.backup_path().exists());
    }

    #[test]
    fn two_corrupt_snapshots_are_preserved_without_crashing_recovery() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        fs::write(store.path(), b"broken primary").unwrap();
        fs::write(store.backup_path(), b"broken backup").unwrap();
        let report = store.load_recovering().unwrap();
        assert!(report.recovery_notice.is_some());
        assert_eq!(fs::read_dir(directory.path()).unwrap().count(), 2);
        assert!(!store.path().exists());
        assert!(!store.backup_path().exists());
    }

    #[test]
    fn future_schema_is_not_quarantined_or_replaced_by_an_older_backup() {
        let directory = tempfile::tempdir().unwrap();
        let store = JsonPersistence::new(directory.path().join("state.json"));
        let future = br#"{"schema_version":999,"future_only":true}"#;
        fs::write(store.path(), future).unwrap();
        fs::write(
            store.backup_path(),
            serde_json::to_vec(&PersistentSnapshot::default()).unwrap(),
        )
        .unwrap();
        assert!(matches!(
            store.load_recovering(),
            Err(AstraError::UnsupportedSchema(999))
        ));
        assert_eq!(fs::read(store.path()).unwrap(), future);
        assert!(store.backup_path().exists());
    }

    #[test]
    fn legacy_migration_preserves_source_and_never_overwrites_a_backup() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = JsonPersistence::new(directory.path().join("old.json"));
        let target = JsonPersistence::new(directory.path().join("new.json"));
        legacy.save(&PersistentSnapshot::default()).unwrap();
        let original = fs::read(legacy.path()).unwrap();
        assert!(JsonPersistence::migrate_legacy_profile(legacy.path(), target.path()).unwrap());
        assert_eq!(fs::read(legacy.path()).unwrap(), original);
        fs::rename(target.path(), target.backup_path()).unwrap();
        assert!(!JsonPersistence::migrate_legacy_profile(legacy.path(), target.path()).unwrap());
        assert!(!target.path().exists());
        assert!(target.load().unwrap().is_some());
    }

    #[test]
    fn invalid_legacy_profile_does_not_create_a_partial_target() {
        let directory = tempfile::tempdir().unwrap();
        let legacy = directory.path().join("old.json");
        let target = directory.path().join("new.json");
        fs::write(&legacy, b"broken").unwrap();
        assert!(JsonPersistence::migrate_legacy_profile(&legacy, &target).is_err());
        assert!(!target.exists());
        assert_eq!(fs::read(legacy).unwrap(), b"broken");
    }

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
