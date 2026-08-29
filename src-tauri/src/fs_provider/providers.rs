//! The `FilesystemProvider` abstraction and its two implementations.
//!
//! Both providers can be asked, uniformly, to describe a resource and to
//! contribute search hits. Provider-specific *mutations* (host mount/unmount,
//! recycle-bin delete, virtual tree generation, …) stay on their own types —
//! the trait only covers what genuinely generalises.

use crate::error::AstraError;
use crate::security::SecurityManager;
use crate::state::SystemState;
use serde::Serialize;

use super::host::{HostFilesystem, HOST_LABEL};
use super::router::AstraLocation;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    Virtual,
    Host,
}

/// A provider-neutral description of one resource.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct EntryView {
    pub display_path: String,
    pub name: String,
    pub kind: ProviderKind,
    pub is_dir: bool,
    pub size: u64,
    pub modified_ms: Option<u64>,
    pub created_ms: Option<u64>,
    pub read_only: bool,
    /// Astra-level access lock (does **not** affect Windows permissions).
    pub astra_locked: bool,
    /// Real host path — populated only for `inspect` on host resources.
    pub host_real_path: Option<String>,
}

/// One search result, tagged with the provider it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchHit {
    pub display: String,
    pub kind: ProviderKind,
}

pub trait FilesystemProvider {
    fn kind(&self) -> ProviderKind;

    /// Scheme label (`ASTRA` / `HOST`).
    fn label(&self) -> &'static str;

    /// Describe a single resource that this provider owns.
    fn describe(&self, location: &AstraLocation) -> Result<EntryView, AstraError>;

    /// Append this provider's matches for `query`.
    fn search(&self, query: &str, hits: &mut Vec<SearchHit>);
}

// ---------------------------------------------------------------------------
// Virtual provider
// ---------------------------------------------------------------------------

pub struct VirtualFilesystemProvider<'a> {
    state: &'a SystemState,
}

impl<'a> VirtualFilesystemProvider<'a> {
    pub fn new(state: &'a SystemState) -> Self {
        Self { state }
    }
}

impl FilesystemProvider for VirtualFilesystemProvider<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Virtual
    }

    fn label(&self) -> &'static str {
        "ASTRA"
    }

    fn describe(&self, location: &AstraLocation) -> Result<EntryView, AstraError> {
        let AstraLocation::Virtual(path) = location else {
            return Err(AstraError::InvalidPath("not a virtual path".to_string()));
        };
        let info = self.state.inspect("ROOT", path)?;
        Ok(EntryView {
            display_path: format!("ASTRA>{}", info.path),
            name: info.metadata.name,
            kind: ProviderKind::Virtual,
            is_dir: matches!(
                info.metadata.resource_type,
                crate::filesystem::ResourceType::Directory
            ),
            size: info.metadata.size,
            modified_ms: Some(info.metadata.modified_at_ms),
            created_ms: Some(info.metadata.created_at_ms),
            read_only: !info.metadata.permissions.write,
            astra_locked: info.metadata.locked,
            host_real_path: None,
        })
    }

    fn search(&self, query: &str, hits: &mut Vec<SearchHit>) {
        // Reuse the already lock-aware virtual search.
        if let Ok(results) = self.state.search("ROOT", "ROOT", query) {
            for hit in results.matches {
                hits.push(SearchHit {
                    display: format!("ASTRA>{}", hit.path),
                    kind: ProviderKind::Virtual,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Host provider
// ---------------------------------------------------------------------------

pub struct HostFilesystemProvider<'a> {
    host: &'a HostFilesystem,
    security: &'a SecurityManager,
}

impl<'a> HostFilesystemProvider<'a> {
    pub fn new(host: &'a HostFilesystem, security: &'a SecurityManager) -> Self {
        Self { host, security }
    }
}

impl FilesystemProvider for HostFilesystemProvider<'_> {
    fn kind(&self) -> ProviderKind {
        ProviderKind::Host
    }

    fn label(&self) -> &'static str {
        HOST_LABEL
    }

    fn describe(&self, location: &AstraLocation) -> Result<EntryView, AstraError> {
        let AstraLocation::Host { mount, relative } = location else {
            return Err(AstraError::InvalidPath("not a host path".to_string()));
        };
        let entry = self.host.entry(mount, relative)?;
        let real = self.host.real_path(mount, relative)?;
        let canonical_id = self.host.canonical_id(mount, relative).unwrap_or_default();
        let ancestor_ids = self.host.ancestor_ids(mount, relative).unwrap_or_default();
        let display_path = if relative.is_empty() {
            format!("{HOST_LABEL}>{mount}")
        } else {
            format!("{HOST_LABEL}>{mount}>{}", relative.join(">"))
        };
        Ok(EntryView {
            display_path,
            name: entry.name,
            kind: ProviderKind::Host,
            is_dir: entry.is_dir,
            size: entry.size,
            modified_ms: entry.modified_ms,
            created_ms: entry.created_ms,
            read_only: entry.read_only,
            astra_locked: ancestor_ids
                .iter()
                .any(|id| self.security.is_host_locked(id))
                || self.security.is_host_locked(&canonical_id),
            host_real_path: Some(real.to_string_lossy().to_string()),
        })
    }

    fn search(&self, query: &str, hits: &mut Vec<SearchHit>) {
        self.host.search(query, hits);
    }
}
