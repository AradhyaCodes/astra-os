use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub type ResourceId = u64;

pub const ROOT_ID: ResourceId = 1;
pub const ROOT_NAME: &str = "ROOT";
pub const DEFAULT_OWNER: &str = "user";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceType {
    Directory,
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    pub read: bool,
    pub write: bool,
    pub execute: bool,
}

impl Permissions {
    pub fn directory_default() -> Self {
        Self {
            read: true,
            write: true,
            execute: true,
        }
    }

    pub fn file_default() -> Self {
        Self {
            read: true,
            write: true,
            execute: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResourceMetadata {
    pub id: ResourceId,
    pub name: String,
    pub resource_type: ResourceType,
    pub created_at_ms: u64,
    pub modified_at_ms: u64,
    pub parent: Option<ResourceId>,
    pub size: u64,
    pub permissions: Permissions,
    pub locked: bool,
    pub owner: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ResourceData {
    Directory {
        children: BTreeMap<String, ResourceId>,
    },
    File {
        /// UTF-8 text payload. Empty when `bytes` holds the file instead.
        content: String,
        /// Raw bytes for files that are not valid UTF-8 text (images, archives,
        /// compiled binaries…). When `Some`, this is the authoritative payload
        /// and `content` is empty. Persisted as base64 to keep the state file
        /// compact; absent in older state files (`serde(default)`).
        #[serde(
            default,
            with = "byte_payload",
            skip_serializing_if = "Option::is_none"
        )]
        bytes: Option<Vec<u8>>,
    },
}

/// Serde adapter: `Option<Vec<u8>>` ⇄ base64 string in the persisted JSON.
mod byte_payload {
    use base64::{engine::general_purpose::STANDARD, Engine as _};
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(
        value: &Option<Vec<u8>>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        match value {
            Some(bytes) => serializer.serialize_str(&STANDARD.encode(bytes)),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Vec<u8>>, D::Error> {
        let encoded = Option::<String>::deserialize(deserializer)?;
        match encoded {
            Some(text) => STANDARD
                .decode(text.as_bytes())
                .map(Some)
                .map_err(serde::de::Error::custom),
            None => Ok(None),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Resource {
    pub metadata: ResourceMetadata,
    pub data: ResourceData,
}

impl Resource {
    pub fn directory(id: ResourceId, name: String, parent: Option<ResourceId>) -> Self {
        let now = unix_time_ms();
        Self {
            metadata: ResourceMetadata {
                id,
                name,
                resource_type: ResourceType::Directory,
                created_at_ms: now,
                modified_at_ms: now,
                parent,
                size: 0,
                permissions: Permissions::directory_default(),
                locked: false,
                owner: DEFAULT_OWNER.to_string(),
            },
            data: ResourceData::Directory {
                children: BTreeMap::new(),
            },
        }
    }

    pub fn file(id: ResourceId, name: String, parent: ResourceId, content: String) -> Self {
        let size = content.len() as u64;
        Self::file_with(
            id,
            name,
            parent,
            size,
            ResourceData::File {
                content,
                bytes: None,
            },
        )
    }

    /// A file whose payload is raw bytes rather than UTF-8 text.
    pub fn file_binary(id: ResourceId, name: String, parent: ResourceId, bytes: Vec<u8>) -> Self {
        let size = bytes.len() as u64;
        Self::file_with(
            id,
            name,
            parent,
            size,
            ResourceData::File {
                content: String::new(),
                bytes: Some(bytes),
            },
        )
    }

    fn file_with(
        id: ResourceId,
        name: String,
        parent: ResourceId,
        size: u64,
        data: ResourceData,
    ) -> Self {
        let now = unix_time_ms();
        Self {
            metadata: ResourceMetadata {
                id,
                name,
                resource_type: ResourceType::File,
                created_at_ms: now,
                modified_at_ms: now,
                parent: Some(parent),
                size,
                permissions: Permissions::file_default(),
                locked: false,
                owner: DEFAULT_OWNER.to_string(),
            },
            data,
        }
    }

    pub fn children(&self) -> Option<&BTreeMap<String, ResourceId>> {
        match &self.data {
            ResourceData::Directory { children } => Some(children),
            ResourceData::File { .. } => None,
        }
    }

    pub fn children_mut(&mut self) -> Option<&mut BTreeMap<String, ResourceId>> {
        match &mut self.data {
            ResourceData::Directory { children } => Some(children),
            ResourceData::File { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VirtualFileSystem {
    pub(crate) resources: BTreeMap<ResourceId, Resource>,
    pub(crate) next_id: ResourceId,
}

impl VirtualFileSystem {
    pub fn new() -> Self {
        let mut filesystem = Self::empty();
        for name in [
            "Desktop",
            "Documents",
            "Downloads",
            "Applications",
            "Games",
            "Pictures",
            "Music",
            "Projects",
            "System",
        ] {
            filesystem
                .insert_directory(ROOT_ID, name.to_string())
                .expect("fixed initial directory names must be valid");
        }
        filesystem
    }

    pub(crate) fn empty() -> Self {
        let mut resources = BTreeMap::new();
        resources.insert(
            ROOT_ID,
            Resource::directory(ROOT_ID, ROOT_NAME.to_string(), None),
        );
        Self {
            resources,
            next_id: ROOT_ID + 1,
        }
    }

    pub(crate) fn allocate_id(&mut self) -> ResourceId {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    /// Combined size of every file payload currently stored.
    pub fn total_file_bytes(&self) -> u64 {
        self.resources
            .values()
            .filter(|resource| resource.metadata.resource_type == ResourceType::File)
            .map(|resource| resource.metadata.size)
            .sum()
    }

    pub(crate) fn insert_directory(
        &mut self,
        parent_id: ResourceId,
        name: String,
    ) -> Result<ResourceId, &'static str> {
        let id = self.allocate_id();
        let parent = self
            .resources
            .get_mut(&parent_id)
            .ok_or("parent resource is missing")?;
        let children = parent
            .children_mut()
            .ok_or("parent resource is not a directory")?;
        if children.contains_key(&name) {
            return Err("duplicate resource name");
        }
        children.insert(name.clone(), id);
        parent.metadata.modified_at_ms = unix_time_ms();
        self.resources
            .insert(id, Resource::directory(id, name, Some(parent_id)));
        Ok(id)
    }
}

impl Default for VirtualFileSystem {
    fn default() -> Self {
        Self::new()
    }
}

pub struct VfsState(pub Arc<RwLock<VirtualFileSystem>>);

pub(crate) fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}
