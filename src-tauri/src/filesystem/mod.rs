//! Aaru-OS — Filesystem module
//!
//! Rust-owned virtual in-memory filesystem with the following rules:
//! - Case-sensitive names
//! - Internal spaces are allowed; no leading/trailing whitespace and no
//!   control characters
//! - No duplicate names within the same directory
//! - Maximum nesting depth of 64
//! - Files carry an extension (`notes.txt`) or a dot-prefixed name (`.env`);
//!   folders have no such requirement
//! - File payloads are UTF-8 text or, for non-text files, raw bytes
//! - Almanac special characters must be escapable
//!
pub mod model;
pub mod operations;
pub mod ops;
pub mod path;
pub mod tree_parser;
pub mod validation;

pub use model::{
    Permissions, Resource, ResourceData, ResourceId, ResourceMetadata, ResourceType, VfsState,
    VirtualFileSystem,
};
pub use operations::{DeleteSummary, ResourceInfo, SearchResults};
