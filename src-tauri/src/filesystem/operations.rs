use super::model::{
    unix_time_ms, Resource, ResourceData, ResourceId, ResourceMetadata, ResourceType,
    VirtualFileSystem, ROOT_ID, ROOT_NAME,
};
use super::path::{components, depth, join, normalize_path, split_path};
use super::tree_parser::{parse_tree, TreeNode};
use super::validation::{ensure_depth, ensure_within_budget, validate_name};
use crate::error::AaruError;
use serde::Serialize;
use std::collections::BTreeSet;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ResourceInfo {
    pub path: String,
    pub metadata: ResourceMetadata,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
pub struct DeleteSummary {
    pub files: u64,
    pub directories: u64,
    pub total_resources: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct SearchResults {
    pub matches: Vec<ResourceInfo>,
    pub skipped_subtrees: Vec<String>,
}

impl VirtualFileSystem {
    pub fn resolve_path(&self, cwd: &str, path: &str) -> Result<ResourceId, AaruError> {
        let canonical = normalize_path(cwd, path)?;
        let mut current_id = ROOT_ID;

        for component in components(&canonical)? {
            let current = self.resource(current_id)?;
            let children = current
                .children()
                .ok_or_else(|| AaruError::NotADirectory(self.path_for_id(current_id)))?;
            current_id = *children
                .get(&component)
                .ok_or_else(|| AaruError::PathNotFound(canonical.clone()))?;
        }

        Ok(current_id)
    }

    pub fn root_directory(&self) -> ResourceInfo {
        self.info_for_id(ROOT_ID)
            .expect("the root resource must always exist")
    }

    pub fn parent_directory(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let resource = self.resource(id)?;
        let parent_id = resource.metadata.parent.unwrap_or(ROOT_ID);
        self.info_for_id(parent_id)
    }

    pub fn open_directory(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let resource = self.resource(id)?;
        if resource.metadata.resource_type != ResourceType::Directory {
            return Err(AaruError::NotADirectory(self.path_for_id(id)));
        }
        self.ensure_readable(resource, &self.path_for_id(id))?;
        self.info_for_id(id)
    }

    pub fn list_directory(&self, cwd: &str, path: &str) -> Result<Vec<ResourceInfo>, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let resource = self.resource(id)?;
        self.ensure_readable(resource, &self.path_for_id(id))?;
        let children = resource
            .children()
            .ok_or_else(|| AaruError::NotADirectory(self.path_for_id(id)))?;

        children
            .values()
            .map(|child_id| self.info_for_id(*child_id))
            .collect()
    }

    pub fn inspect(&self, cwd: &str, path: &str) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        self.info_for_id(id)
    }

    pub fn create_directory(&mut self, cwd: &str, path: &str) -> Result<ResourceInfo, AaruError> {
        let canonical = normalize_path(cwd, path)?;
        let (parent_path, name) = split_path(&canonical)?;
        validate_name(&name, ResourceType::Directory)?;
        ensure_depth(&canonical, depth(&canonical)?)?;
        let parent_id = self.resolve_path(ROOT_NAME, &parent_path)?;
        let id = self.create_directory_in(parent_id, &name, &parent_path)?;
        self.info_for_id(id)
    }

    pub fn create_file(
        &mut self,
        cwd: &str,
        path: &str,
        content: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let canonical = normalize_path(cwd, path)?;
        let (parent_path, name) = split_path(&canonical)?;
        validate_name(&name, ResourceType::File)?;
        ensure_depth(&canonical, depth(&canonical)?)?;
        ensure_within_budget(self.total_file_bytes(), content.len() as u64)?;
        let parent_id = self.resolve_path(ROOT_NAME, &parent_path)?;
        let id = self.create_file_in(parent_id, &name, content, &parent_path)?;
        self.info_for_id(id)
    }

    /// Create a new file whose payload is raw bytes rather than UTF-8 text.
    pub fn create_file_bytes(
        &mut self,
        cwd: &str,
        path: &str,
        data: &[u8],
    ) -> Result<ResourceInfo, AaruError> {
        let canonical = normalize_path(cwd, path)?;
        let (parent_path, name) = split_path(&canonical)?;
        validate_name(&name, ResourceType::File)?;
        ensure_depth(&canonical, depth(&canonical)?)?;
        ensure_within_budget(self.total_file_bytes(), data.len() as u64)?;
        let parent_id = self.resolve_path(ROOT_NAME, &parent_path)?;
        self.ensure_directory_writable(parent_id)?;
        self.ensure_child_name_available(parent_id, &name, &parent_path)?;
        let id = self.allocate_id();
        let parent = self.resource_mut(parent_id)?;
        parent
            .children_mut()
            .ok_or_else(|| AaruError::NotADirectory(parent_path.clone()))?
            .insert(name.clone(), id);
        parent.metadata.modified_at_ms = unix_time_ms();
        self.resources.insert(
            id,
            Resource::file_binary(id, name, parent_id, data.to_vec()),
        );
        self.info_for_id(id)
    }

    pub fn create_tree_atomic(
        &mut self,
        cwd: &str,
        expression: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let tree = parse_tree(expression)?;
        let mut staged = self.clone();
        let parent_id = staged.resolve_path(cwd, ".")?;
        let parent_path = staged.path_for_id(parent_id);
        let id = staged.create_tree_node(parent_id, &parent_path, &tree)?;
        let result = staged.info_for_id(id)?;
        *self = staged;
        Ok(result)
    }

    pub fn rename(
        &mut self,
        cwd: &str,
        path: &str,
        new_name: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        if id == ROOT_ID {
            return Err(AaruError::PermissionDenied(
                "the root directory cannot be renamed".to_string(),
            ));
        }

        let resource_type = self.resource(id)?.metadata.resource_type;
        validate_name(new_name, resource_type)?;
        let parent_id =
            self.resource(id)?.metadata.parent.ok_or_else(|| {
                AaruError::Filesystem("resource has no parent directory".to_string())
            })?;
        self.ensure_directory_writable(parent_id)?;

        let old_name = self.resource(id)?.metadata.name.clone();
        if old_name == new_name {
            return self.info_for_id(id);
        }
        let parent_path = self.path_for_id(parent_id);
        if self
            .resource(parent_id)?
            .children()
            .is_some_and(|children| children.contains_key(new_name))
        {
            return Err(AaruError::DuplicateName {
                name: new_name.to_string(),
                dir: parent_path,
            });
        }

        let parent = self.resource_mut(parent_id)?;
        let children = parent.children_mut().ok_or_else(|| {
            AaruError::NotADirectory("parent resource is not a directory".to_string())
        })?;
        children.remove(&old_name);
        children.insert(new_name.to_string(), id);
        parent.metadata.modified_at_ms = unix_time_ms();

        let resource = self.resource_mut(id)?;
        resource.metadata.name = new_name.to_string();
        resource.metadata.modified_at_ms = unix_time_ms();
        self.info_for_id(id)
    }

    pub fn move_resource(
        &mut self,
        cwd: &str,
        source_path: &str,
        destination_directory: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let mut staged = self.clone();
        let id = staged.move_resource_inner(cwd, source_path, destination_directory)?;
        let result = staged.info_for_id(id)?;
        *self = staged;
        Ok(result)
    }

    pub fn copy_resource(
        &mut self,
        cwd: &str,
        source_path: &str,
        destination_directory: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let mut staged = self.clone();
        let source_id = staged.resolve_path(cwd, source_path)?;
        if source_id == ROOT_ID {
            return Err(AaruError::PermissionDenied(
                "the root directory cannot be copied".to_string(),
            ));
        }
        let destination_id = staged.resolve_path(cwd, destination_directory)?;
        staged.ensure_directory_writable(destination_id)?;
        staged.ensure_not_self_or_descendant(source_id, destination_id, "copy")?;

        let source = staged.resource(source_id)?.clone();
        let destination_path = staged.path_for_id(destination_id);
        staged.ensure_child_name_available(
            destination_id,
            &source.metadata.name,
            &destination_path,
        )?;
        staged.ensure_subtree_fits(source_id, destination_id)?;

        let copied_id = staged.copy_subtree(source_id, destination_id)?;
        let result = staged.info_for_id(copied_id)?;
        *self = staged;
        Ok(result)
    }

    pub fn delete_preview(&self, cwd: &str, path: &str) -> Result<DeleteSummary, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        if id == ROOT_ID {
            return Err(AaruError::PermissionDenied(
                "the root directory cannot be deleted".to_string(),
            ));
        }
        self.summarize_subtree(id)
    }

    pub fn delete_recursive(&mut self, cwd: &str, path: &str) -> Result<DeleteSummary, AaruError> {
        let mut staged = self.clone();
        let id = staged.resolve_path(cwd, path)?;
        if id == ROOT_ID {
            return Err(AaruError::PermissionDenied(
                "the root directory cannot be deleted".to_string(),
            ));
        }
        staged.ensure_subtree_deletable(id)?;
        let summary = staged.summarize_subtree(id)?;
        let resource = staged.resource(id)?.clone();
        let parent_id = resource
            .metadata
            .parent
            .ok_or_else(|| AaruError::Filesystem("resource has no parent directory".to_string()))?;
        staged.ensure_directory_writable(parent_id)?;
        let ids = staged.collect_subtree_ids(id)?;

        let parent_path = staged.path_for_id(parent_id);
        let parent = staged.resource_mut(parent_id)?;
        parent
            .children_mut()
            .ok_or(AaruError::NotADirectory(parent_path))?
            .remove(&resource.metadata.name);
        parent.metadata.modified_at_ms = unix_time_ms();
        for resource_id in ids {
            staged.resources.remove(&resource_id);
        }

        *self = staged;
        Ok(summary)
    }

    pub fn search(
        &self,
        cwd: &str,
        start_path: &str,
        query: &str,
        skip_inaccessible: bool,
    ) -> Result<SearchResults, AaruError> {
        if query.is_empty() {
            return Err(AaruError::InvalidArgument(
                "search query cannot be empty".to_string(),
            ));
        }
        let start_id = self.resolve_path(cwd, start_path)?;
        let mut results = SearchResults::default();
        self.search_subtree(start_id, query, skip_inaccessible, &mut results)?;
        Ok(results)
    }

    pub fn read_file(&self, cwd: &str, path: &str) -> Result<String, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let resource = self.resource(id)?;
        self.ensure_readable(resource, &self.path_for_id(id))?;
        match &resource.data {
            ResourceData::File {
                content,
                bytes: None,
            } => Ok(content.clone()),
            ResourceData::File { bytes: Some(_), .. } => Err(AaruError::InvalidArgument(format!(
                "{} holds binary data — use 'almanac reveal' or open it on the host",
                self.path_for_id(id)
            ))),
            ResourceData::Directory { .. } => Err(AaruError::NotAFile(self.path_for_id(id))),
        }
    }

    /// Read a file's payload as raw bytes — text files as their UTF-8 bytes,
    /// binary files as their stored bytes.
    pub fn read_file_bytes(&self, cwd: &str, path: &str) -> Result<Vec<u8>, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let resource = self.resource(id)?;
        self.ensure_readable(resource, &self.path_for_id(id))?;
        match &resource.data {
            ResourceData::File {
                bytes: Some(bytes), ..
            } => Ok(bytes.clone()),
            ResourceData::File {
                content,
                bytes: None,
            } => Ok(content.clone().into_bytes()),
            ResourceData::Directory { .. } => Err(AaruError::NotAFile(self.path_for_id(id))),
        }
    }

    pub fn write_file(
        &mut self,
        cwd: &str,
        path: &str,
        content: &str,
    ) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let path = self.path_for_id(id);
        let previous_size = {
            let resource = self.resource(id)?;
            self.ensure_writable(resource, &path)?;
            if resource.metadata.resource_type != ResourceType::File {
                return Err(AaruError::NotAFile(path));
            }
            resource.metadata.size
        };
        ensure_within_budget(
            self.total_file_bytes().saturating_sub(previous_size),
            content.len() as u64,
        )?;

        let resource = self.resource_mut(id)?;
        let ResourceData::File {
            content: existing,
            bytes,
        } = &mut resource.data
        else {
            return Err(AaruError::NotAFile(path));
        };
        *existing = content.to_string();
        *bytes = None;
        resource.metadata.size = content.len() as u64;
        resource.metadata.modified_at_ms = unix_time_ms();
        self.info_for_id(id)
    }

    /// Replace an existing file's payload with raw bytes.
    pub fn write_file_bytes(
        &mut self,
        cwd: &str,
        path: &str,
        data: &[u8],
    ) -> Result<ResourceInfo, AaruError> {
        let id = self.resolve_path(cwd, path)?;
        let path = self.path_for_id(id);
        let previous_size = {
            let resource = self.resource(id)?;
            self.ensure_writable(resource, &path)?;
            if resource.metadata.resource_type != ResourceType::File {
                return Err(AaruError::NotAFile(path));
            }
            resource.metadata.size
        };
        ensure_within_budget(
            self.total_file_bytes().saturating_sub(previous_size),
            data.len() as u64,
        )?;

        let resource = self.resource_mut(id)?;
        let ResourceData::File {
            content: existing,
            bytes,
        } = &mut resource.data
        else {
            return Err(AaruError::NotAFile(path));
        };
        existing.clear();
        *bytes = Some(data.to_vec());
        resource.metadata.size = data.len() as u64;
        resource.metadata.modified_at_ms = unix_time_ms();
        self.info_for_id(id)
    }

    fn move_resource_inner(
        &mut self,
        cwd: &str,
        source_path: &str,
        destination_directory: &str,
    ) -> Result<ResourceId, AaruError> {
        let source_id = self.resolve_path(cwd, source_path)?;
        if source_id == ROOT_ID {
            return Err(AaruError::PermissionDenied(
                "the root directory cannot be moved".to_string(),
            ));
        }
        let destination_id = self.resolve_path(cwd, destination_directory)?;
        let source = self.resource(source_id)?.clone();
        let old_parent_id = source
            .metadata
            .parent
            .ok_or_else(|| AaruError::Filesystem("resource has no parent directory".to_string()))?;
        if old_parent_id == destination_id {
            return Ok(source_id);
        }

        self.ensure_directory_writable(old_parent_id)?;
        self.ensure_directory_writable(destination_id)?;
        self.ensure_not_self_or_descendant(source_id, destination_id, "move")?;
        let destination_path = self.path_for_id(destination_id);
        self.ensure_child_name_available(destination_id, &source.metadata.name, &destination_path)?;
        self.ensure_subtree_fits(source_id, destination_id)?;

        let old_parent_path = self.path_for_id(old_parent_id);
        let old_parent = self.resource_mut(old_parent_id)?;
        old_parent
            .children_mut()
            .ok_or(AaruError::NotADirectory(old_parent_path))?
            .remove(&source.metadata.name);
        old_parent.metadata.modified_at_ms = unix_time_ms();

        let destination = self.resource_mut(destination_id)?;
        destination
            .children_mut()
            .ok_or_else(|| AaruError::NotADirectory(destination_path.clone()))?
            .insert(source.metadata.name.clone(), source_id);
        destination.metadata.modified_at_ms = unix_time_ms();

        let moved = self.resource_mut(source_id)?;
        moved.metadata.parent = Some(destination_id);
        moved.metadata.modified_at_ms = unix_time_ms();
        Ok(source_id)
    }

    fn create_tree_node(
        &mut self,
        parent_id: ResourceId,
        parent_path: &str,
        node: &TreeNode,
    ) -> Result<ResourceId, AaruError> {
        validate_name(&node.name, ResourceType::Directory)?;
        let path = join(parent_path, &node.name);
        ensure_depth(&path, depth(&path)?)?;
        let id = self.create_directory_in(parent_id, &node.name, parent_path)?;
        for child in &node.children {
            self.create_tree_node(id, &path, child)?;
        }
        Ok(id)
    }

    fn create_directory_in(
        &mut self,
        parent_id: ResourceId,
        name: &str,
        parent_path: &str,
    ) -> Result<ResourceId, AaruError> {
        self.ensure_directory_writable(parent_id)?;
        self.ensure_child_name_available(parent_id, name, parent_path)?;
        let id = self.allocate_id();
        let parent = self.resource_mut(parent_id)?;
        parent
            .children_mut()
            .ok_or_else(|| AaruError::NotADirectory(parent_path.to_string()))?
            .insert(name.to_string(), id);
        parent.metadata.modified_at_ms = unix_time_ms();
        self.resources.insert(
            id,
            Resource::directory(id, name.to_string(), Some(parent_id)),
        );
        Ok(id)
    }

    fn create_file_in(
        &mut self,
        parent_id: ResourceId,
        name: &str,
        content: &str,
        parent_path: &str,
    ) -> Result<ResourceId, AaruError> {
        self.ensure_directory_writable(parent_id)?;
        self.ensure_child_name_available(parent_id, name, parent_path)?;
        let id = self.allocate_id();
        let parent = self.resource_mut(parent_id)?;
        parent
            .children_mut()
            .ok_or_else(|| AaruError::NotADirectory(parent_path.to_string()))?
            .insert(name.to_string(), id);
        parent.metadata.modified_at_ms = unix_time_ms();
        self.resources.insert(
            id,
            Resource::file(id, name.to_string(), parent_id, content.to_string()),
        );
        Ok(id)
    }

    fn copy_subtree(
        &mut self,
        source_id: ResourceId,
        destination_parent_id: ResourceId,
    ) -> Result<ResourceId, AaruError> {
        let source = self.resource(source_id)?.clone();
        let new_id = self.allocate_id();
        let now = unix_time_ms();
        let mut copy = source.clone();
        copy.metadata.id = new_id;
        copy.metadata.parent = Some(destination_parent_id);
        copy.metadata.created_at_ms = now;
        copy.metadata.modified_at_ms = now;
        if matches!(copy.data, ResourceData::Directory { .. }) {
            copy.data = ResourceData::Directory {
                children: Default::default(),
            };
        }

        let destination_path = self.path_for_id(destination_parent_id);
        let destination = self.resource_mut(destination_parent_id)?;
        destination
            .children_mut()
            .ok_or(AaruError::NotADirectory(destination_path))?
            .insert(copy.metadata.name.clone(), new_id);
        destination.metadata.modified_at_ms = now;
        self.resources.insert(new_id, copy);

        if let ResourceData::Directory { children } = source.data {
            for child_id in children.values() {
                self.copy_subtree(*child_id, new_id)?;
            }
        }
        Ok(new_id)
    }

    fn ensure_subtree_fits(
        &self,
        source_id: ResourceId,
        destination_id: ResourceId,
    ) -> Result<(), AaruError> {
        let destination_path = self.path_for_id(destination_id);
        let destination_depth = depth(&destination_path)?;
        let target_depth = destination_depth + 1 + self.subtree_height(source_id)?;
        let target_path = join(&destination_path, &self.resource(source_id)?.metadata.name);
        ensure_depth(&target_path, target_depth)
    }

    fn subtree_height(&self, id: ResourceId) -> Result<usize, AaruError> {
        let resource = self.resource(id)?;
        let Some(children) = resource.children() else {
            return Ok(0);
        };
        let mut height = 0;
        for child_id in children.values() {
            height = height.max(1 + self.subtree_height(*child_id)?);
        }
        Ok(height)
    }

    fn ensure_not_self_or_descendant(
        &self,
        source_id: ResourceId,
        destination_id: ResourceId,
        operation: &str,
    ) -> Result<(), AaruError> {
        if self.resource(source_id)?.metadata.resource_type != ResourceType::Directory {
            return Ok(());
        }
        let mut cursor = Some(destination_id);
        while let Some(id) = cursor {
            if id == source_id {
                return Err(AaruError::InvalidMove(format!(
                    "cannot {operation} a directory into itself or one of its descendants"
                )));
            }
            cursor = self.resource(id)?.metadata.parent;
        }
        Ok(())
    }

    fn summarize_subtree(&self, id: ResourceId) -> Result<DeleteSummary, AaruError> {
        let resource = self.resource(id)?;
        let mut summary = match resource.metadata.resource_type {
            ResourceType::File => DeleteSummary {
                files: 1,
                directories: 0,
                total_resources: 1,
                total_bytes: resource.metadata.size,
            },
            ResourceType::Directory => DeleteSummary {
                files: 0,
                directories: 1,
                total_resources: 1,
                total_bytes: 0,
            },
        };
        if let Some(children) = resource.children() {
            for child_id in children.values() {
                let child = self.summarize_subtree(*child_id)?;
                summary.files += child.files;
                summary.directories += child.directories;
                summary.total_resources += child.total_resources;
                summary.total_bytes += child.total_bytes;
            }
        }
        Ok(summary)
    }

    fn ensure_subtree_deletable(&self, id: ResourceId) -> Result<(), AaruError> {
        let resource = self.resource(id)?;
        self.ensure_writable(resource, &self.path_for_id(id))?;
        if let Some(children) = resource.children() {
            for child_id in children.values() {
                self.ensure_subtree_deletable(*child_id)?;
            }
        }
        Ok(())
    }

    fn collect_subtree_ids(&self, id: ResourceId) -> Result<Vec<ResourceId>, AaruError> {
        let mut ids = Vec::new();
        if let Some(children) = self.resource(id)?.children() {
            for child_id in children.values() {
                ids.extend(self.collect_subtree_ids(*child_id)?);
            }
        }
        ids.push(id);
        Ok(ids)
    }

    fn search_subtree(
        &self,
        id: ResourceId,
        query: &str,
        skip_inaccessible: bool,
        results: &mut SearchResults,
    ) -> Result<(), AaruError> {
        let resource = self.resource(id)?;
        let path = self.path_for_id(id);
        if resource.metadata.locked || !resource.metadata.permissions.read {
            if skip_inaccessible {
                results.skipped_subtrees.push(path);
                return Ok(());
            }
            return Err(AaruError::PermissionDenied(format!(
                "cannot search inaccessible resource: {path}"
            )));
        }
        if resource.metadata.name.contains(query) {
            results.matches.push(self.info_for_id(id)?);
        }
        if let Some(children) = resource.children() {
            for child_id in children.values() {
                self.search_subtree(*child_id, query, skip_inaccessible, results)?;
            }
        }
        Ok(())
    }

    pub(crate) fn resource_by_id(&self, id: ResourceId) -> Result<&Resource, AaruError> {
        self.resource(id)
    }

    pub(crate) fn resource_mut_by_id(
        &mut self,
        id: ResourceId,
    ) -> Result<&mut Resource, AaruError> {
        self.resource_mut(id)
    }

    pub(crate) fn resource_info(&self, id: ResourceId) -> Result<ResourceInfo, AaruError> {
        self.info_for_id(id)
    }

    pub(crate) fn resource_path(&self, id: ResourceId) -> String {
        self.path_for_id(id)
    }

    pub(crate) fn ancestor_ids(&self, id: ResourceId) -> Result<Vec<ResourceId>, AaruError> {
        let mut ids = Vec::new();
        let mut cursor = Some(id);
        while let Some(resource_id) = cursor {
            ids.push(resource_id);
            cursor = self.resource(resource_id)?.metadata.parent;
        }
        ids.reverse();
        Ok(ids)
    }

    pub(crate) fn subtree_ids(&self, id: ResourceId) -> Result<Vec<ResourceId>, AaruError> {
        self.collect_subtree_ids(id)
    }

    pub(crate) fn existing_ids(&self) -> BTreeSet<ResourceId> {
        self.resources.keys().copied().collect()
    }

    pub(crate) fn parallel_subtree_pairs(
        &self,
        source_id: ResourceId,
        copied_id: ResourceId,
    ) -> Result<Vec<(ResourceId, ResourceId)>, AaruError> {
        let mut pairs = vec![(source_id, copied_id)];
        let source = self.resource(source_id)?;
        let copied = self.resource(copied_id)?;
        if let (Some(source_children), Some(copied_children)) =
            (source.children(), copied.children())
        {
            for (name, source_child_id) in source_children {
                let copied_child_id = copied_children.get(name).ok_or_else(|| {
                    AaruError::Filesystem(format!("copied subtree is missing resource '{name}'"))
                })?;
                pairs.extend(self.parallel_subtree_pairs(*source_child_id, *copied_child_id)?);
            }
        }
        Ok(pairs)
    }

    fn ensure_directory_writable(&self, id: ResourceId) -> Result<(), AaruError> {
        let resource = self.resource(id)?;
        let path = self.path_for_id(id);
        if resource.metadata.resource_type != ResourceType::Directory {
            return Err(AaruError::NotADirectory(path));
        }
        self.ensure_writable(resource, &path)
    }

    fn ensure_readable(&self, resource: &Resource, path: &str) -> Result<(), AaruError> {
        if !resource.metadata.permissions.read {
            return Err(AaruError::PermissionDenied(format!(
                "resource is not readable: {path}"
            )));
        }
        Ok(())
    }

    fn ensure_writable(&self, resource: &Resource, path: &str) -> Result<(), AaruError> {
        if !resource.metadata.permissions.write {
            return Err(AaruError::PermissionDenied(format!(
                "resource is not writable: {path}"
            )));
        }
        Ok(())
    }

    fn ensure_child_name_available(
        &self,
        parent_id: ResourceId,
        name: &str,
        parent_path: &str,
    ) -> Result<(), AaruError> {
        let parent = self.resource(parent_id)?;
        let children = parent
            .children()
            .ok_or_else(|| AaruError::NotADirectory(parent_path.to_string()))?;
        if children.contains_key(name) {
            return Err(AaruError::DuplicateName {
                name: name.to_string(),
                dir: parent_path.to_string(),
            });
        }
        Ok(())
    }

    fn resource(&self, id: ResourceId) -> Result<&Resource, AaruError> {
        self.resources.get(&id).ok_or_else(|| {
            AaruError::Filesystem(format!("resource ID {id} is missing from the filesystem"))
        })
    }

    fn resource_mut(&mut self, id: ResourceId) -> Result<&mut Resource, AaruError> {
        self.resources.get_mut(&id).ok_or_else(|| {
            AaruError::Filesystem(format!("resource ID {id} is missing from the filesystem"))
        })
    }

    fn info_for_id(&self, id: ResourceId) -> Result<ResourceInfo, AaruError> {
        let resource = self.resource(id)?;
        Ok(ResourceInfo {
            path: self.path_for_id(id),
            metadata: resource.metadata.clone(),
        })
    }

    fn path_for_id(&self, id: ResourceId) -> String {
        if id == ROOT_ID {
            return ROOT_NAME.to_string();
        }

        let mut names = Vec::new();
        let mut cursor = Some(id);
        while let Some(resource_id) = cursor {
            let Some(resource) = self.resources.get(&resource_id) else {
                break;
            };
            if resource_id != ROOT_ID {
                names.push(resource.metadata.name.clone());
            }
            cursor = resource.metadata.parent;
        }
        names.reverse();
        if names.is_empty() {
            ROOT_NAME.to_string()
        } else {
            names
                .iter()
                .fold(ROOT_NAME.to_string(), |path, name| join(&path, name))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_filesystem_matches_phase_one_specification() {
        let filesystem = VirtualFileSystem::new();
        let names: Vec<String> = filesystem
            .list_directory("ROOT", ".")
            .unwrap()
            .into_iter()
            .map(|info| info.metadata.name)
            .collect();
        assert_eq!(
            names,
            vec![
                "Applications",
                "Desktop",
                "Documents",
                "Downloads",
                "Games",
                "Music",
                "Pictures",
                "Projects",
                "System",
            ]
        );
    }

    #[test]
    fn enforces_case_sensitivity_duplicates_names_and_extensions() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem.create_directory("ROOT", "TEST").unwrap();
        filesystem.create_directory("ROOT", "test").unwrap();
        assert!(filesystem.create_directory("ROOT", "test").is_err());
        // Internal spaces are allowed now.
        filesystem.create_directory("ROOT", "My Project").unwrap();
        assert!(filesystem.create_file("ROOT", "README", "").is_err());
        filesystem.create_file("ROOT", "README.txt", "ok").unwrap();
        filesystem
            .create_file("ROOT", "notes v2.txt", "ok")
            .unwrap();
        // Dot-prefixed names (dotfiles) are valid files even without an extension.
        filesystem.create_file("ROOT", ".env", "KEY=1").unwrap();
        filesystem
            .create_file("ROOT", ".gitignore", "target")
            .unwrap();
        filesystem
            .create_file("ROOT", ".env.local", "KEY=2")
            .unwrap();
        assert!(filesystem.create_file("ROOT", "trailing.", "").is_err());
        assert!(filesystem.create_directory("ROOT", "...").is_err());
    }

    #[test]
    fn binary_files_round_trip_through_reads_writes_and_persistence() {
        let mut filesystem = VirtualFileSystem::new();
        let blob: Vec<u8> = vec![0, 159, 146, 150, 255, 0, 1, 2];

        filesystem
            .create_file_bytes("ROOT", "Downloads>icon.bin", &blob)
            .unwrap();
        assert_eq!(
            filesystem
                .read_file_bytes("ROOT", "Downloads>icon.bin")
                .unwrap(),
            blob
        );
        // Reading a binary payload as text is refused.
        assert!(filesystem.read_file("ROOT", "Downloads>icon.bin").is_err());

        // Survives a JSON serialise / deserialise cycle, stored compactly as
        // base64 rather than a JSON array of integers.
        let json = serde_json::to_string(&filesystem).unwrap();
        assert!(json.contains("AJ+Slv8AAQI="));
        assert!(!json.contains("[0,159,146,150,255,0,1,2]"));
        let restored: VirtualFileSystem = serde_json::from_str(&json).unwrap();
        assert_eq!(
            restored
                .read_file_bytes("ROOT", "Downloads>icon.bin")
                .unwrap(),
            blob
        );

        // Rewriting with text clears the binary payload.
        filesystem
            .write_file("ROOT", "Downloads>icon.bin", "now text")
            .unwrap();
        assert_eq!(
            filesystem.read_file("ROOT", "Downloads>icon.bin").unwrap(),
            "now text"
        );
    }

    #[test]
    fn accepts_the_max_depth_and_rejects_one_deeper() {
        use crate::filesystem::validation::MAX_DEPTH;
        let mut filesystem = VirtualFileSystem::new();

        // `depth` counts segments below ROOT, so MAX_DEPTH nested dirs are valid.
        let mut path = String::new();
        for level in 1..=MAX_DEPTH {
            if level > 1 {
                path.push('>');
            }
            path.push_str(&format!("d{level}"));
            filesystem.create_directory("ROOT", &path).unwrap();
        }
        assert!(filesystem.inspect("ROOT", &path).is_ok());

        // One level past the cap is rejected.
        assert!(filesystem
            .create_directory("ROOT", &format!("{path}>overflow"))
            .is_err());
    }

    #[test]
    fn atomic_tree_generation_rolls_back_on_duplicate_child() {
        let mut filesystem = VirtualFileSystem::new();
        assert!(filesystem
            .create_tree_atomic("ROOT", "Workspace>(Frontend,Frontend)")
            .is_err());
        assert!(filesystem.inspect("ROOT", "Workspace").is_err());
    }

    #[test]
    fn parser_special_characters_can_be_used_in_names() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", r"Releases>(Stable\>2026,Docs\,Archive)")
            .unwrap();
        assert!(filesystem
            .inspect("ROOT", r"Projects>Releases>Stable\>2026")
            .is_ok());
        assert!(filesystem
            .inspect("ROOT", "Projects>Releases>Docs,Archive")
            .is_ok());
    }

    #[test]
    fn rename_rejects_conflicts_and_preserves_metadata_id() {
        let mut filesystem = VirtualFileSystem::new();
        let original = filesystem
            .create_directory("ROOT>Projects", "Alpha")
            .unwrap();
        filesystem
            .create_directory("ROOT>Projects", "Beta")
            .unwrap();
        assert!(filesystem.rename("ROOT>Projects", "Alpha", "Beta").is_err());
        let renamed = filesystem
            .rename("ROOT>Projects", "Alpha", "Gamma")
            .unwrap();
        assert_eq!(original.metadata.id, renamed.metadata.id);
        assert_eq!(renamed.path, "ROOT>Projects>Gamma");
    }

    #[test]
    fn copies_directories_recursively_without_overwriting() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", "Source>(Frontend,Backend)")
            .unwrap();
        filesystem
            .create_file("ROOT>Projects>Source>Frontend", "app.ts", "code")
            .unwrap();
        filesystem
            .copy_resource("ROOT", "Projects>Source", "Documents")
            .unwrap();
        assert_eq!(
            filesystem
                .read_file("ROOT", "Documents>Source>Frontend>app.ts")
                .unwrap(),
            "code"
        );
        assert!(filesystem
            .copy_resource("ROOT", "Projects>Source", "Documents")
            .is_err());
    }

    #[test]
    fn moves_resources_and_blocks_descendant_cycles() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", "Source>(Nested)")
            .unwrap();
        assert!(filesystem
            .move_resource("ROOT", "Projects>Source", "Projects>Source>Nested")
            .is_err());
        filesystem
            .move_resource("ROOT", "Projects>Source", "Documents")
            .unwrap();
        assert!(filesystem
            .inspect("ROOT", "Documents>Source>Nested")
            .is_ok());
        assert!(filesystem.inspect("ROOT", "Projects>Source").is_err());
    }

    #[test]
    fn recursive_delete_reports_counts_before_and_after_commit() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", "DeleteMe>(One,Two)")
            .unwrap();
        filesystem
            .create_file("ROOT>Projects>DeleteMe>One", "note.txt", "hello")
            .unwrap();
        let preview = filesystem
            .delete_preview("ROOT", "Projects>DeleteMe")
            .unwrap();
        assert_eq!(preview.directories, 3);
        assert_eq!(preview.files, 1);
        assert_eq!(preview.total_resources, 4);
        assert_eq!(preview.total_bytes, 5);
        assert_eq!(
            filesystem
                .delete_recursive("ROOT", "Projects>DeleteMe")
                .unwrap(),
            preview
        );
        assert!(filesystem.inspect("ROOT", "Projects>DeleteMe").is_err());
    }

    #[test]
    fn resolves_paths_searches_recursively_and_inspects_metadata() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", "AaruOS>(Frontend,Backend)")
            .unwrap();
        let file = filesystem
            .create_file("ROOT>Projects>AaruOS>Backend", "server.rs", "fn main() {}")
            .unwrap();
        let resolved = filesystem
            .open_directory("ROOT>Projects", "AaruOS>Backend")
            .unwrap();
        assert_eq!(resolved.path, "ROOT>Projects>AaruOS>Backend");
        let results = filesystem.search("ROOT", "Projects", "Aaru", true).unwrap();
        assert_eq!(results.matches.len(), 1);
        let inspected = filesystem
            .inspect("ROOT", "Projects>AaruOS>Backend>server.rs")
            .unwrap();
        assert_eq!(inspected.metadata.id, file.metadata.id);
        assert_eq!(inspected.metadata.size, 12);
        assert_eq!(inspected.metadata.owner, "user");
    }

    #[test]
    fn search_can_skip_inaccessible_subtrees() {
        let mut filesystem = VirtualFileSystem::new();
        filesystem
            .create_tree_atomic("ROOT>Projects", "Public>(FindMe)")
            .unwrap();
        filesystem
            .create_tree_atomic("ROOT>Projects", "Private>(FindMeToo)")
            .unwrap();
        let private_id = filesystem.resolve_path("ROOT", "Projects>Private").unwrap();
        filesystem
            .resources
            .get_mut(&private_id)
            .unwrap()
            .metadata
            .locked = true;

        let results = filesystem
            .search("ROOT", "Projects", "FindMe", true)
            .unwrap();
        assert_eq!(results.matches.len(), 1);
        assert_eq!(results.skipped_subtrees, vec!["ROOT>Projects>Private"]);
        assert!(filesystem
            .search("ROOT", "Projects", "FindMe", false)
            .is_err());
    }
}
