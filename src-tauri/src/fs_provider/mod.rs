//! Aaru-OS — Filesystem provider abstraction (Phase 4).
//!
//! ```text
//! FilesystemProvider
//! ├── VirtualFilesystemProvider   (the in-memory Aaru filesystem)
//! └── HostFilesystemProvider      (selected, explicitly mounted Windows folders)
//! ```
//!
//! Aaru virtual paths and Windows host paths stay logically separate:
//!
//! * `Documents>Projects`            → [`VirtualFilesystemProvider`]
//! * `HOST>Documents>Report.docx`    → [`HostFilesystemProvider`]
//!
//! The routing decision is made once, in Rust, by [`router::route`]. React
//! never sees a raw host path and never decides which provider to use.

pub mod host;
pub mod providers;
pub mod router;

pub use host::{HostFilesystem, HostMountRecord, MountView, HOST_LABEL};
pub use providers::{
    EntryView, FilesystemProvider, HostFilesystemProvider, ProviderKind, SearchHit,
    VirtualFilesystemProvider,
};
pub use router::{route, AaruLocation};
