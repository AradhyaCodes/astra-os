use super::model::{ResourceType, ROOT_NAME};
use crate::error::AstraError;

pub const MAX_DEPTH: u8 = 64;

/// Ceiling on the combined size of every file payload in the virtual
/// filesystem. The whole VFS is held in memory and rewritten to a single JSON
/// document on every mutation, so unbounded growth makes every later command
/// slow. Writes that would push the total past this are rejected.
pub const MAX_TOTAL_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Reject a write that would take the filesystem past [`MAX_TOTAL_FILE_BYTES`].
/// `current_total` is the sum of all file sizes now; `delta` is how much this
/// write adds (new size minus the size it replaces).
pub fn ensure_within_budget(current_total: u64, delta: u64) -> Result<(), AstraError> {
    if current_total.saturating_add(delta) > MAX_TOTAL_FILE_BYTES {
        return Err(AstraError::Filesystem(format!(
            "virtual filesystem is full — {} MiB limit reached (delete files or copy fewer in)",
            MAX_TOTAL_FILE_BYTES / (1024 * 1024)
        )));
    }
    Ok(())
}

pub fn validate_name(name: &str, resource_type: ResourceType) -> Result<(), AstraError> {
    if name.is_empty() {
        return invalid_name(name, "name cannot be empty");
    }
    if name != name.trim() {
        return invalid_name(name, "name cannot start or end with whitespace");
    }
    if name.chars().any(|character| character.is_control()) {
        return invalid_name(name, "name cannot contain control characters");
    }
    if matches!(name, "." | "..") || name == ROOT_NAME {
        return invalid_name(name, "name is reserved by the filesystem");
    }
    if name.chars().all(|character| character == '.') {
        return invalid_name(name, "name cannot be made up entirely of dots");
    }
    if resource_type == ResourceType::File {
        if name.ends_with('.') {
            return invalid_name(name, "file names cannot end with '.'");
        }
        // A file is valid if it either carries an extension (`notes.txt`) or is
        // a dot-prefixed name with no extension (`.env`, `.gitignore`).
        let is_dotfile = name.starts_with('.') && name.len() > 1;
        let has_extension = name.trim_start_matches('.').contains('.');
        if !is_dotfile && !has_extension {
            return invalid_name(
                name,
                "files need an extension (notes.txt) or a dot-prefixed name (.env)",
            );
        }
    }
    Ok(())
}

pub fn ensure_depth(path: &str, depth: usize) -> Result<(), AstraError> {
    if depth > usize::from(MAX_DEPTH) {
        return Err(AstraError::MaxDepthExceeded {
            max: MAX_DEPTH,
            path: path.to_string(),
        });
    }
    Ok(())
}

fn invalid_name<T>(name: &str, reason: &str) -> Result<T, AstraError> {
    Err(AstraError::InvalidName {
        name: name.to_string(),
        reason: reason.to_string(),
    })
}
