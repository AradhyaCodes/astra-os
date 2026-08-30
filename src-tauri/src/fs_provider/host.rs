//! The host filesystem bridge.
//!
//! A [`HostFilesystem`] holds a table of **explicitly approved** mount roots
//! (canonicalised Windows directories). Every access is:
//!
//! 1. logically routed (`HOST>alias>rel…`) by [`super::router`],
//! 2. joined onto the mount root,
//! 3. re-checked so the real, canonical path still lives under that root
//!    (defeating `..`, prefix confusion and symlink/junction escape for the
//!    parts that already exist).
//!
//! Astra's *virtual* naming rules (extension-or-dotfile names, depth cap, …)
//! are **not** applied to pre-existing host resources. New host resources are
//! validated against Windows naming rules only.

use crate::error::AstraError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

/// Scheme label that selects the host provider.
pub const HOST_LABEL: &str = "HOST";

/// Recursion / result guards so a `lookout` over a huge tree stays bounded.
const MAX_SEARCH_DEPTH: usize = 16;
const MAX_SEARCH_HITS: usize = 2000;

/// Real user directories, injected from Tauri's path resolver so this module
/// stays testable and free of hardcoded usernames.
#[derive(Debug, Clone, Default)]
pub struct HostDirs {
    pub desktop: Option<PathBuf>,
    pub documents: Option<PathBuf>,
    pub downloads: Option<PathBuf>,
    pub home: Option<PathBuf>,
    /// `%PUBLIC%\Desktop` — the all-users Desktop where system-wide app
    /// shortcuts live. Windows Explorer shows this merged with the user's own
    /// Desktop; the Astra bridge keeps it a separate, explicit mount.
    pub public_desktop: Option<PathBuf>,
}

/// One approved mount.
#[derive(Debug, Clone)]
pub struct HostMount {
    pub alias: String,
    /// Canonical, absolute directory this mount is pinned to.
    pub root: PathBuf,
    /// The path as originally chosen (for display).
    pub source: String,
    pub is_default: bool,
}

/// Serialisable record of a *user* mount, persisted across restarts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostMountRecord {
    pub alias: String,
    pub path: String,
}

/// What `almanac mount` / `mounts` shows the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MountView {
    pub alias: String,
    pub source: String,
    pub is_default: bool,
    pub available: bool,
}

/// A single host directory entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostEntry {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub created_ms: Option<u64>,
    pub read_only: bool,
}

/// Result of a host "delete".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostDeleteOutcome {
    pub files: u64,
    pub folders: u64,
    pub recycled: bool,
}

#[derive(Debug, Default, Clone)]
pub struct HostFilesystem {
    mounts: BTreeMap<String, HostMount>,
}

impl HostFilesystem {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build the standard set of safe user mounts that actually exist.
    pub fn with_defaults(dirs: &HostDirs) -> Self {
        let mut filesystem = Self::new();
        filesystem.install_defaults(dirs);
        filesystem
    }

    /// Restore persisted user mounts, then (re)install the default mounts.
    pub fn restore(records: &[HostMountRecord], dirs: &HostDirs) -> Self {
        let mut filesystem = Self::new();
        for record in records {
            let path = PathBuf::from(&record.path);
            if let Ok(canonical) = dunce::canonicalize(&path) {
                if canonical.is_dir() {
                    let alias = filesystem.unique_alias(sanitize_alias(&record.alias));
                    filesystem.mounts.insert(
                        alias.clone(),
                        HostMount {
                            alias,
                            root: canonical,
                            source: record.path.clone(),
                            is_default: false,
                        },
                    );
                }
            }
        }
        filesystem.install_defaults(dirs);
        filesystem
    }

    fn install_defaults(&mut self, dirs: &HostDirs) {
        let candidates = [
            ("Desktop", dirs.desktop.clone()),
            ("PublicDesktop", dirs.public_desktop.clone()),
            ("Documents", dirs.documents.clone()),
            ("Downloads", dirs.downloads.clone()),
            (
                "Projects",
                dirs.home.as_ref().map(|home| home.join("Projects")),
            ),
        ];
        for (alias, path) in candidates {
            let Some(path) = path else { continue };
            // Never *create* a missing default (spec: don't auto-create Projects).
            let Ok(canonical) = dunce::canonicalize(&path) else {
                continue;
            };
            if !canonical.is_dir() || self.mounts.contains_key(alias) {
                continue;
            }
            self.mounts.insert(
                alias.to_string(),
                HostMount {
                    alias: alias.to_string(),
                    root: canonical,
                    source: path.display().to_string(),
                    is_default: true,
                },
            );
        }
    }

    // ------------------------------------------------------------------
    // Mount table management
    // ------------------------------------------------------------------

    /// Approve a new directory. `source` must resolve to a real directory; the
    /// alias is derived from it (or `requested_alias`) and made unique
    /// deterministically on collision.
    pub fn mount(
        &mut self,
        source: &Path,
        requested_alias: Option<&str>,
    ) -> Result<String, AstraError> {
        let canonical = dunce::canonicalize(source)
            .map_err(|error| AstraError::PathNotFound(format!("{}: {error}", source.display())))?;
        if !canonical.is_dir() {
            return Err(AstraError::NotADirectory(canonical.display().to_string()));
        }

        let base = requested_alias
            .map(sanitize_alias)
            .filter(|alias| !alias.is_empty())
            .or_else(|| {
                canonical
                    .file_name()
                    .map(|name| sanitize_alias(&name.to_string_lossy()))
            })
            .filter(|alias| !alias.is_empty())
            .unwrap_or_else(|| "Mount".to_string());

        // Re-mounting the exact same directory is idempotent.
        if let Some(existing) = self.mounts.values().find(|mount| mount.root == canonical) {
            return Ok(existing.alias.clone());
        }

        let alias = self.unique_alias(base);
        self.mounts.insert(
            alias.clone(),
            HostMount {
                alias: alias.clone(),
                root: canonical,
                source: source.display().to_string(),
                is_default: false,
            },
        );
        Ok(alias)
    }

    pub fn unmount(&mut self, alias: &str) -> Result<(), AstraError> {
        if self.mounts.remove(alias).is_none() {
            return Err(AstraError::PathNotFound(format!(
                "no host mount named '{alias}'"
            )));
        }
        Ok(())
    }

    pub fn list_mounts(&self) -> Vec<MountView> {
        self.mounts
            .values()
            .map(|mount| MountView {
                alias: mount.alias.clone(),
                source: mount.source.clone(),
                is_default: mount.is_default,
                available: mount.root.is_dir(),
            })
            .collect()
    }

    /// User (non-default) mounts, for persistence.
    pub fn user_records(&self) -> Vec<HostMountRecord> {
        self.mounts
            .values()
            .filter(|mount| !mount.is_default)
            .map(|mount| HostMountRecord {
                alias: mount.alias.clone(),
                path: mount.root.display().to_string(),
            })
            .collect()
    }

    pub fn mount_aliases(&self) -> Vec<String> {
        self.mounts.keys().cloned().collect()
    }

    /// Best-effort `HOST>alias>rel…` label for a stored canonical path id.
    pub fn display_for_id(&self, canonical_id: &str) -> String {
        let candidate = PathBuf::from(canonical_id);
        for mount in self.mounts.values() {
            if let Ok(relative) = candidate.strip_prefix(&mount.root) {
                let rel: Vec<String> = relative
                    .components()
                    .filter_map(|component| match component {
                        Component::Normal(part) => Some(part.to_string_lossy().to_string()),
                        _ => None,
                    })
                    .collect();
                return display(&mount.alias, &rel);
            }
        }
        format!("{HOST_LABEL}>(unmounted)")
    }

    fn unique_alias(&self, base: String) -> String {
        if !self.mounts.contains_key(&base) {
            return base;
        }
        (2..)
            .map(|suffix| format!("{base}-{suffix}"))
            .find(|candidate| !self.mounts.contains_key(candidate))
            .expect("an unused alias always exists")
    }

    // ------------------------------------------------------------------
    // Path resolution + traversal guard
    // ------------------------------------------------------------------

    fn mount_of(&self, alias: &str) -> Result<&HostMount, AstraError> {
        self.mounts.get(alias).ok_or_else(|| {
            AstraError::PathNotFound(format!(
                "no host mount named '{alias}' (try 'almanac mount')"
            ))
        })
    }

    /// Join `relative` onto the mount root and verify the real, canonical path
    /// does not escape it. Returns the (possibly not-yet-existing) target path.
    pub fn resolve(&self, alias: &str, relative: &[String]) -> Result<PathBuf, AstraError> {
        let mount = self.mount_of(alias)?;
        let mut target = mount.root.clone();
        for segment in relative {
            if segment.is_empty()
                || segment == "."
                || segment == ".."
                || segment.contains(['/', '\\', ':'])
            {
                return Err(AstraError::InvalidPath(format!(
                    "illegal host path segment '{segment}'"
                )));
            }
            target.push(segment);
        }

        // Canonicalise the longest existing prefix and confirm containment.
        let anchor = longest_existing_ancestor(&target);
        let canonical_anchor = dunce::canonicalize(&anchor).map_err(|error| {
            AstraError::Filesystem(format!(
                "could not canonicalise {}: {error}",
                anchor.display()
            ))
        })?;
        if !canonical_anchor.starts_with(&mount.root) {
            return Err(AstraError::PermissionDenied(format!(
                "path escapes the approved mount root {HOST_LABEL}>{alias}"
            )));
        }
        // Reject `..` components that survived (belt and braces).
        if target
            .components()
            .any(|component| matches!(component, Component::ParentDir))
        {
            return Err(AstraError::PermissionDenied(
                "'..' is not allowed in host paths".to_string(),
            ));
        }
        Ok(target)
    }

    /// Stable canonical identifier for lock metadata.
    pub fn canonical_id(&self, alias: &str, relative: &[String]) -> Result<String, AstraError> {
        let target = self.resolve(alias, relative)?;
        let canonical = match dunce::canonicalize(&target) {
            Ok(path) => path,
            Err(_) => {
                let parent = target.parent().unwrap_or(&target);
                let parent = dunce::canonicalize(parent).map_err(|error| {
                    AstraError::Filesystem(format!("could not canonicalise parent: {error}"))
                })?;
                match target.file_name() {
                    Some(name) => parent.join(name),
                    None => parent,
                }
            }
        };
        Ok(canonical.to_string_lossy().to_string())
    }

    /// Canonical id for every mount-relative ancestor of a host path, closest
    /// last. Used to find applicable Astra locks.
    pub fn ancestor_ids(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<Vec<String>, AstraError> {
        let mut ids = Vec::new();
        for depth in 0..=relative.len() {
            ids.push(self.canonical_id(alias, &relative[..depth])?);
        }
        Ok(ids)
    }

    // ------------------------------------------------------------------
    // Operations
    // ------------------------------------------------------------------

    pub fn entry(&self, alias: &str, relative: &[String]) -> Result<HostEntry, AstraError> {
        let path = self.resolve(alias, relative)?;
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| AstraError::PathNotFound(format!("{}: {error}", path.display())))?;
        Ok(describe(&path, &metadata))
    }

    pub fn real_path(&self, alias: &str, relative: &[String]) -> Result<PathBuf, AstraError> {
        self.resolve(alias, relative)
    }

    pub fn list_dir(&self, alias: &str, relative: &[String]) -> Result<Vec<HostEntry>, AstraError> {
        let path = self.resolve(alias, relative)?;
        if !path.is_dir() {
            return Err(AstraError::NotADirectory(display(alias, relative)));
        }
        let mut entries = Vec::new();
        for item in fs::read_dir(&path)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))?
        {
            let item = item.map_err(|error| AstraError::Filesystem(error.to_string()))?;
            if let Ok(metadata) = item.metadata() {
                entries.push(describe(&item.path(), &metadata));
            }
        }
        entries.sort_by_key(|entry| entry.name.to_lowercase());
        Ok(entries)
    }

    pub fn read_text(&self, alias: &str, relative: &[String]) -> Result<String, AstraError> {
        let path = self.resolve(alias, relative)?;
        if !path.is_file() {
            return Err(AstraError::NotAFile(display(alias, relative)));
        }
        let bytes = fs::read(&path)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))?;
        String::from_utf8(bytes).map_err(|_| {
            AstraError::InvalidArgument(format!(
                "{} is not UTF-8 text and cannot be shown here",
                display(alias, relative)
            ))
        })
    }

    /// Read a host file as raw bytes, no UTF-8 requirement.
    pub fn read_bytes(&self, alias: &str, relative: &[String]) -> Result<Vec<u8>, AstraError> {
        let path = self.resolve(alias, relative)?;
        if !path.is_file() {
            return Err(AstraError::NotAFile(display(alias, relative)));
        }
        fs::read(&path)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))
    }

    pub fn write_text(
        &self,
        alias: &str,
        relative: &[String],
        contents: &str,
        must_exist: bool,
    ) -> Result<HostEntry, AstraError> {
        self.write_raw(alias, relative, contents.as_bytes(), must_exist)
    }

    /// Write raw bytes to a host file (no UTF-8 requirement).
    pub fn write_bytes(
        &self,
        alias: &str,
        relative: &[String],
        data: &[u8],
        must_exist: bool,
    ) -> Result<HostEntry, AstraError> {
        self.write_raw(alias, relative, data, must_exist)
    }

    fn write_raw(
        &self,
        alias: &str,
        relative: &[String],
        data: &[u8],
        must_exist: bool,
    ) -> Result<HostEntry, AstraError> {
        let (_, leaf) = split_leaf(relative)?;
        validate_windows_name(leaf)?;
        let path = self.resolve(alias, relative)?;
        if must_exist && !path.is_file() {
            return Err(AstraError::PathNotFound(format!(
                "{} — rewrite requires an existing host file",
                display(alias, relative)
            )));
        }
        if path.is_dir() {
            return Err(AstraError::NotAFile(display(alias, relative)));
        }
        if let Some(parent) = path.parent() {
            if !parent.is_dir() {
                return Err(AstraError::PathNotFound(format!(
                    "parent directory of {} does not exist",
                    display(alias, relative)
                )));
            }
        }
        // Creation must never truncate an existing destination during a copy.
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(!must_exist)
            .truncate(must_exist)
            .open(&path)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))?;
        std::io::Write::write_all(&mut file, data)
            .and_then(|()| file.sync_all())
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))?;
        self.entry(alias, relative)
    }

    pub fn create_dir(&self, alias: &str, relative: &[String]) -> Result<HostEntry, AstraError> {
        let (_, leaf) = split_leaf(relative)?;
        validate_windows_name(leaf)?;
        let path = self.resolve(alias, relative)?;
        if path.exists() {
            return Err(AstraError::DuplicateName {
                name: leaf.to_string(),
                dir: display(alias, &relative[..relative.len().saturating_sub(1)]),
            });
        }
        fs::create_dir(&path)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", path.display())))?;
        self.entry(alias, relative)
    }

    pub fn rename(
        &self,
        alias: &str,
        relative: &[String],
        new_name: &str,
    ) -> Result<HostEntry, AstraError> {
        if relative.is_empty() {
            return Err(AstraError::PermissionDenied(
                "a mount root cannot be renamed".to_string(),
            ));
        }
        validate_windows_name(new_name)?;
        let source = self.resolve(alias, relative)?;
        if !source.exists() {
            return Err(AstraError::PathNotFound(display(alias, relative)));
        }
        let parent_rel = &relative[..relative.len().saturating_sub(1)];
        let mut target_rel = parent_rel.to_vec();
        target_rel.push(new_name.to_string());
        let target = self.resolve(alias, &target_rel)?;
        if target.exists() {
            return Err(AstraError::DuplicateName {
                name: new_name.to_string(),
                dir: display(alias, parent_rel),
            });
        }
        fs::rename(&source, &target)
            .map_err(|error| AstraError::Filesystem(format!("rename failed: {error}")))?;
        self.entry(alias, &target_rel)
    }

    /// Host→host move or copy. `destination_relative` must be an existing
    /// directory inside the same mount table.
    pub fn relocate(
        &self,
        from_alias: &str,
        from_relative: &[String],
        to_alias: &str,
        to_relative: &[String],
        copy: bool,
    ) -> Result<HostEntry, AstraError> {
        if from_relative.is_empty() {
            return Err(AstraError::PermissionDenied(
                "a mount root cannot be moved or copied".to_string(),
            ));
        }
        let source = self.resolve(from_alias, from_relative)?;
        if !source.exists() {
            return Err(AstraError::PathNotFound(display(from_alias, from_relative)));
        }
        let destination_dir = self.resolve(to_alias, to_relative)?;
        if !destination_dir.is_dir() {
            return Err(AstraError::NotADirectory(display(to_alias, to_relative)));
        }
        let name = source
            .file_name()
            .ok_or_else(|| AstraError::InvalidPath("source has no file name".to_string()))?
            .to_os_string();
        let target = destination_dir.join(&name);
        if target.exists() {
            return Err(AstraError::DuplicateName {
                name: name.to_string_lossy().to_string(),
                dir: display(to_alias, to_relative),
            });
        }
        // Prevent moving a directory into itself / its own subtree.
        let canonical_source = dunce::canonicalize(&source)
            .map_err(|error| AstraError::Filesystem(error.to_string()))?;
        let canonical_destination = dunce::canonicalize(&destination_dir)
            .map_err(|error| AstraError::Filesystem(error.to_string()))?;
        if source.is_dir() && canonical_destination.starts_with(&canonical_source) {
            return Err(AstraError::InvalidMove(
                "cannot move a directory into itself".to_string(),
            ));
        }

        if copy {
            copy_recursive(&source, &target)?;
        } else if fs::rename(&source, &target).is_err() {
            // Cross-volume rename fails; fall back to copy + recycle of source.
            copy_recursive(&source, &target)?;
            trash::delete(&source).map_err(|error| {
                AstraError::Filesystem(format!(
                    "moved by copy, but could not recycle the original: {error}"
                ))
            })?;
        }

        let mut target_rel = to_relative.to_vec();
        target_rel.push(name.to_string_lossy().to_string());
        self.entry(to_alias, &target_rel)
    }

    pub fn count_descendants(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<(u64, u64), AstraError> {
        let path = self.resolve(alias, relative)?;
        if !path.exists() {
            return Err(AstraError::PathNotFound(display(alias, relative)));
        }
        let mut files = 0;
        let mut folders = 0;
        count_into(&path, &mut files, &mut folders);
        Ok((files, folders))
    }

    /// The host "delete": move the target to the Windows Recycle Bin. Never a
    /// permanent recursive delete — if recycling fails we return the error.
    pub fn recycle(
        &self,
        alias: &str,
        relative: &[String],
    ) -> Result<HostDeleteOutcome, AstraError> {
        let path = self.resolve(alias, relative)?;
        if !path.exists() {
            return Err(AstraError::PathNotFound(display(alias, relative)));
        }
        if relative.is_empty() {
            return Err(AstraError::PermissionDenied(
                "a mount root itself cannot be deleted — unmount it instead".to_string(),
            ));
        }
        let (files, folders) = self.count_descendants(alias, relative)?;
        trash::delete(&path).map_err(|error| {
            AstraError::Filesystem(format!(
                "could not move {} to the Recycle Bin: {error} — nothing was deleted",
                display(alias, relative)
            ))
        })?;
        Ok(HostDeleteOutcome {
            files,
            folders,
            recycled: true,
        })
    }

    // ------------------------------------------------------------------
    // Search
    // ------------------------------------------------------------------

    pub fn search(&self, query: &str, hits: &mut Vec<super::providers::SearchHit>) {
        let needle = query.to_lowercase();
        for mount in self.mounts.values() {
            if hits.len() >= MAX_SEARCH_HITS {
                break;
            }
            search_dir(&mount.root, &mount.alias, &mut Vec::new(), &needle, 0, hits);
        }
    }
}

// ---------------------------------------------------------------------------
// Free helpers
// ---------------------------------------------------------------------------

fn display(alias: &str, relative: &[String]) -> String {
    if relative.is_empty() {
        format!("{HOST_LABEL}>{alias}")
    } else {
        format!("{HOST_LABEL}>{alias}>{}", relative.join(">"))
    }
}

fn split_leaf(relative: &[String]) -> Result<(&[String], &str), AstraError> {
    match relative.split_last() {
        Some((leaf, parent)) => Ok((parent, leaf.as_str())),
        None => Err(AstraError::InvalidPath(
            "a host operation needs a target inside the mount, not the mount root".to_string(),
        )),
    }
}

fn longest_existing_ancestor(path: &Path) -> PathBuf {
    let mut current = path.to_path_buf();
    loop {
        if current.exists() {
            return current;
        }
        match current.parent() {
            Some(parent) if parent != current => current = parent.to_path_buf(),
            _ => return current,
        }
    }
}

fn system_time_ms(time: std::io::Result<SystemTime>) -> Option<u64> {
    time.ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_millis().min(u128::from(u64::MAX)) as u64)
}

fn describe(path: &Path, metadata: &fs::Metadata) -> HostEntry {
    HostEntry {
        name: path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
        is_dir: metadata.is_dir(),
        size: if metadata.is_dir() { 0 } else { metadata.len() },
        modified_ms: system_time_ms(metadata.modified()),
        created_ms: system_time_ms(metadata.created()),
        read_only: metadata.permissions().readonly(),
    }
}

fn count_into(path: &Path, files: &mut u64, folders: &mut u64) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() && !is_link_or_reparse(&metadata) {
        *folders += 1;
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                count_into(&entry.path(), files, folders);
            }
        }
    } else {
        *files += 1;
    }
}

fn copy_recursive(source: &Path, target: &Path) -> Result<(), AstraError> {
    copy_tree(source, target, 0)
}

fn is_link_or_reparse(metadata: &fs::Metadata) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    }
    #[cfg(not(windows))]
    {
        metadata.file_type().is_symlink()
    }
}

fn copy_tree(source: &Path, target: &Path, depth: usize) -> Result<(), AstraError> {
    if depth > 64 {
        return Err(AstraError::InvalidMove(
            "host copy exceeds the 64-level depth limit".to_string(),
        ));
    }
    let metadata = fs::symlink_metadata(source)
        .map_err(|error| AstraError::Filesystem(format!("{}: {error}", source.display())))?;
    if is_link_or_reparse(&metadata) {
        return Err(AstraError::PermissionDenied(
            "recursive copies do not follow symbolic links, junctions, or reparse points"
                .to_string(),
        ));
    }
    if metadata.is_dir() {
        fs::create_dir(target)
            .map_err(|error| AstraError::Filesystem(format!("{}: {error}", target.display())))?;
        for entry in
            fs::read_dir(source).map_err(|error| AstraError::Filesystem(error.to_string()))?
        {
            let entry = entry.map_err(|error| AstraError::Filesystem(error.to_string()))?;
            copy_tree(&entry.path(), &target.join(entry.file_name()), depth + 1)?;
        }
    } else if metadata.is_file() {
        let mut input = fs::File::open(source)
            .map_err(|error| AstraError::Filesystem(format!("copy failed: {error}")))?;
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(target)
            .map_err(|error| AstraError::Filesystem(format!("copy failed: {error}")))?;
        std::io::copy(&mut input, &mut output)
            .and_then(|_| output.sync_all())
            .map_err(|error| AstraError::Filesystem(format!("copy failed: {error}")))?;
    } else {
        return Err(AstraError::InvalidMove(
            "host copy only accepts regular files and directories".to_string(),
        ));
    }
    Ok(())
}

fn search_dir(
    dir: &Path,
    alias: &str,
    relative: &mut Vec<String>,
    needle: &str,
    depth: usize,
    hits: &mut Vec<super::providers::SearchHit>,
) {
    if depth > MAX_SEARCH_DEPTH || hits.len() >= MAX_SEARCH_HITS {
        return;
    }
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if hits.len() >= MAX_SEARCH_HITS {
            return;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        relative.push(name.clone());
        if name.to_lowercase().contains(needle) {
            hits.push(super::providers::SearchHit {
                display: format!("{HOST_LABEL}>{alias}>{}", relative.join(">")),
                kind: super::providers::ProviderKind::Host,
            });
        }
        if fs::symlink_metadata(entry.path())
            .map(|metadata| metadata.is_dir() && !is_link_or_reparse(&metadata))
            .unwrap_or(false)
        {
            search_dir(&entry.path(), alias, relative, needle, depth + 1, hits);
        }
        relative.pop();
    }
}

fn sanitize_alias(raw: &str) -> String {
    let cleaned: String = raw
        .trim()
        .chars()
        .map(|character| {
            if character.is_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .collect();
    cleaned.trim_matches(['_', '.', '-']).to_string()
}

/// Windows filename rules — deliberately *not* Astra's virtual rules.
pub fn validate_windows_name(name: &str) -> Result<(), AstraError> {
    if name.is_empty() {
        return Err(AstraError::InvalidName {
            name: name.to_string(),
            reason: "name cannot be empty".to_string(),
        });
    }
    if name
        .chars()
        .any(|c| "<>:\"/\\|?*".contains(c) || (c as u32) < 0x20)
    {
        return Err(AstraError::InvalidName {
            name: name.to_string(),
            reason: r#"Windows names cannot contain < > : " / \ | ? * or control characters"#
                .to_string(),
        });
    }
    if name.ends_with('.') || name.ends_with(' ') {
        return Err(AstraError::InvalidName {
            name: name.to_string(),
            reason: "Windows names cannot end with a space or a dot".to_string(),
        });
    }
    let stem = name.split('.').next().unwrap_or(name).to_ascii_uppercase();
    const RESERVED: [&str; 22] = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if RESERVED.contains(&stem.as_str()) {
        return Err(AstraError::InvalidName {
            name: name.to_string(),
            reason: format!("'{stem}' is a reserved Windows device name"),
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creating_a_host_file_never_overwrites_existing_content() {
        let (dir, host, alias) = temp_mount();
        let relative = vec!["top level notes.txt".to_string()];
        assert!(host
            .write_bytes(&alias, &relative, b"replacement", false)
            .is_err());
        assert_eq!(
            fs::read(dir.path().join("top level notes.txt")).unwrap(),
            b"hello"
        );
        host.write_bytes(&alias, &relative, b"explicit rewrite", true)
            .unwrap();
        assert_eq!(
            fs::read(dir.path().join("top level notes.txt")).unwrap(),
            b"explicit rewrite"
        );
    }

    #[test]
    fn mount_roots_cannot_be_renamed_or_relocated() {
        let (dir, host, alias) = temp_mount();
        assert!(host.rename(&alias, &[], "renamed").is_err());
        assert!(host
            .relocate(&alias, &[], &alias, &["University".to_string()], false)
            .is_err());
        assert!(dir.path().join("top level notes.txt").exists());
    }

    #[test]
    fn recursive_copy_preserves_an_existing_destination() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.txt");
        let target = dir.path().join("target.txt");
        fs::write(&source, b"source").unwrap();
        fs::write(&target, b"original").unwrap();
        assert!(copy_recursive(&source, &target).is_err());
        assert_eq!(fs::read(target).unwrap(), b"original");
    }

    #[cfg(windows)]
    #[test]
    fn recursive_copy_refuses_a_junction_inside_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir(&source).unwrap();
        fs::write(outside.path().join("private.txt"), b"not part of the mount").unwrap();
        let junction = source.join("junction");
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&junction)
            .arg(outside.path())
            .output()
            .unwrap();
        assert!(output.status.success(), "could not create test junction");
        let target = dir.path().join("target");
        assert!(matches!(
            copy_recursive(&source, &target),
            Err(AstraError::PermissionDenied(_))
        ));
        assert!(!target.join("junction").join("private.txt").exists());
        fs::remove_dir(&junction).unwrap();
        assert!(outside.path().join("private.txt").exists());
    }

    fn temp_mount() -> (tempfile::TempDir, HostFilesystem, String) {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("University")).unwrap();
        std::fs::write(dir.path().join("University").join("report.pdf"), b"x").unwrap();
        std::fs::write(dir.path().join("top level notes.txt"), b"hello").unwrap();
        let mut host = HostFilesystem::new();
        let alias = host.mount(dir.path(), Some("Dev")).unwrap();
        (dir, host, alias)
    }

    #[test]
    fn mounts_are_deduplicated_deterministically() {
        let dir = tempfile::tempdir().unwrap();
        let sub_a = dir.path().join("a");
        let sub_b = dir.path().join("b");
        std::fs::create_dir(&sub_a).unwrap();
        std::fs::create_dir(&sub_b).unwrap();
        let mut host = HostFilesystem::new();
        assert_eq!(host.mount(&sub_a, Some("Work")).unwrap(), "Work");
        assert_eq!(host.mount(&sub_b, Some("Work")).unwrap(), "Work-2");
        // Same directory again → same alias, no third mount.
        assert_eq!(host.mount(&sub_a, Some("Work")).unwrap(), "Work");
        assert_eq!(host.list_mounts().len(), 2);
    }

    #[test]
    fn default_mounts_include_the_public_desktop_when_it_exists() {
        let user_desktop = tempfile::tempdir().unwrap();
        let public_desktop = tempfile::tempdir().unwrap();
        let host = HostFilesystem::with_defaults(&HostDirs {
            desktop: Some(user_desktop.path().to_path_buf()),
            public_desktop: Some(public_desktop.path().to_path_buf()),
            ..HostDirs::default()
        });
        let aliases = host.mount_aliases();
        assert!(aliases.contains(&"Desktop".to_string()));
        assert!(aliases.contains(&"PublicDesktop".to_string()));
        // A missing public desktop is simply skipped (never created).
        let host2 = HostFilesystem::with_defaults(&HostDirs {
            public_desktop: Some(public_desktop.path().join("does-not-exist")),
            ..HostDirs::default()
        });
        assert!(!host2.mount_aliases().contains(&"PublicDesktop".to_string()));
    }

    #[test]
    fn unmount_removes_a_mount() {
        let (_dir, mut host, alias) = temp_mount();
        host.unmount(&alias).unwrap();
        assert!(host.unmount(&alias).is_err());
        assert!(host.list_dir(&alias, &[]).is_err());
    }

    #[test]
    fn traversal_outside_the_mount_is_rejected() {
        let (_dir, host, alias) = temp_mount();
        assert!(matches!(
            host.resolve(&alias, &["..".to_string(), "Windows".to_string()]),
            Err(AstraError::InvalidPath(_))
        ));
        assert!(matches!(
            host.resolve(&alias, &["sub\\..\\..".to_string()]),
            Err(AstraError::InvalidPath(_))
        ));
    }

    #[test]
    fn host_names_keep_spaces_and_missing_extensions() {
        let (_dir, host, alias) = temp_mount();
        // Pre-existing file with spaces is inspectable.
        let entry = host
            .entry(&alias, &["top level notes.txt".to_string()])
            .unwrap();
        assert_eq!(entry.name, "top level notes.txt");
        assert!(!entry.is_dir);
        // Creating a new extensionless file is allowed under Windows rules.
        host.write_text(&alias, &["Makefile".to_string()], "all:\n", false)
            .unwrap();
        assert!(host.entry(&alias, &["Makefile".to_string()]).is_ok());
        // …but a reserved device name is refused.
        assert!(host
            .write_text(&alias, &["CON".to_string()], "", false)
            .is_err());
    }

    #[test]
    fn inspect_move_and_copy_on_host() {
        let (_dir, host, alias) = temp_mount();
        std::fs::create_dir(host.resolve(&alias, &["Archive".to_string()]).unwrap()).unwrap();

        host.relocate(
            &alias,
            &["University".to_string(), "report.pdf".to_string()],
            &alias,
            &["Archive".to_string()],
            true,
        )
        .unwrap();
        assert!(host
            .entry(&alias, &["Archive".to_string(), "report.pdf".to_string()])
            .is_ok());
        assert!(host
            .entry(
                &alias,
                &["University".to_string(), "report.pdf".to_string()]
            )
            .is_ok());

        host.relocate(
            &alias,
            &["University".to_string(), "report.pdf".to_string()],
            &alias,
            &["Archive".to_string()],
            false,
        )
        .unwrap_err(); // name already exists in Archive
    }

    #[test]
    fn count_descendants_walks_the_subtree() {
        let (_dir, host, alias) = temp_mount();
        let (files, folders) = host
            .count_descendants(&alias, &["University".to_string()])
            .unwrap();
        assert_eq!(files, 1);
        assert_eq!(folders, 1);
    }

    #[test]
    fn search_matches_across_the_mount() {
        let (_dir, host, alias) = temp_mount();
        let mut hits = Vec::new();
        host.search("report", &mut hits);
        assert!(hits
            .iter()
            .any(|hit| hit.display == format!("HOST>{alias}>University>report.pdf")));
    }
}
