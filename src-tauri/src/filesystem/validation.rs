use super::model::{ResourceData, ResourceType, VirtualFileSystem, ROOT_ID, ROOT_NAME};
use crate::error::AstraError;
use std::collections::BTreeSet;

pub const MAX_DEPTH: u8 = 64;

/// Ceiling on the combined size of every file payload in the virtual
/// filesystem. The whole VFS is held in memory and rewritten to a single JSON
/// document on every mutation, so unbounded growth makes every later command
/// slow. Writes that would push the total past this are rejected.
pub const MAX_TOTAL_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// Validate untrusted on-disk structure without calling recursive runtime
/// operations, which assume a connected, acyclic tree and consistent counters.
pub(crate) fn validate_snapshot(filesystem: &VirtualFileSystem) -> Result<(), AstraError> {
    let corrupt = |reason: &str| AstraError::CorruptPersistence(reason.to_string());
    let max_id = filesystem
        .resources
        .keys()
        .next_back()
        .copied()
        .unwrap_or(0);
    if filesystem.next_id <= max_id || filesystem.next_id == u64::MAX {
        return Err(corrupt("invalid resource ID counter"));
    }
    let mut pending = vec![(ROOT_ID, None, 0usize)];
    let mut seen = BTreeSet::new();
    let mut total_bytes = 0u64;
    while let Some((id, parent, depth)) = pending.pop() {
        if depth > usize::from(MAX_DEPTH) || !seen.insert(id) {
            return Err(corrupt(
                "filesystem has a cycle, duplicate link, or excessive depth",
            ));
        }
        let resource = filesystem
            .resources
            .get(&id)
            .ok_or_else(|| corrupt("filesystem references a missing resource"))?;
        if resource.metadata.id != id || resource.metadata.parent != parent {
            return Err(corrupt("resource identity or parent is inconsistent"));
        }
        if id == ROOT_ID {
            if resource.metadata.name != ROOT_NAME
                || resource.metadata.resource_type != ResourceType::Directory
            {
                return Err(corrupt("invalid filesystem root"));
            }
        } else {
            validate_name(&resource.metadata.name, resource.metadata.resource_type)
                .map_err(|_| corrupt("invalid persisted resource name"))?;
        }
        match (&resource.data, resource.metadata.resource_type) {
            (ResourceData::Directory { children }, ResourceType::Directory) => {
                for (name, child_id) in children {
                    let child = filesystem
                        .resources
                        .get(child_id)
                        .ok_or_else(|| corrupt("directory references a missing child"))?;
                    if name != &child.metadata.name {
                        return Err(corrupt("directory child name is inconsistent"));
                    }
                    pending.push((*child_id, Some(id), depth + 1));
                }
            }
            (ResourceData::File { content, bytes }, ResourceType::File) => {
                if bytes.is_some() && !content.is_empty() {
                    return Err(corrupt(
                        "file contains conflicting text and binary payloads",
                    ));
                }
                let size = bytes.as_ref().map_or(content.len(), Vec::len) as u64;
                if resource.metadata.size != size {
                    return Err(corrupt("persisted file size is inconsistent"));
                }
                total_bytes = total_bytes
                    .checked_add(size)
                    .ok_or_else(|| corrupt("persisted file size overflow"))?;
                if total_bytes > MAX_TOTAL_FILE_BYTES {
                    return Err(corrupt(
                        "persisted virtual filesystem exceeds its payload budget",
                    ));
                }
            }
            _ => return Err(corrupt("resource type does not match its data")),
        }
    }
    if seen.len() != filesystem.resources.len() {
        return Err(corrupt("filesystem contains unreachable resources"));
    }
    Ok(())
}

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
