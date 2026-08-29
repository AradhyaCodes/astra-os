//! Astra OS — Error types
//!
//! Central error enum for all Astra OS subsystems. Each variant maps to a
//! specific subsystem failure. `thiserror` derives `Display` and `Error`
//! implementations automatically.

use thiserror::Error;

/// Top-level error type for the Astra OS runtime.
#[derive(Debug, Error)]
pub enum AstraError {
    // ------------------------------------------------------------------
    // Kernel / Configuration
    // ------------------------------------------------------------------
    #[error("kernel configuration error: {0}")]
    KernelConfig(String),

    // ------------------------------------------------------------------
    // Filesystem
    // ------------------------------------------------------------------
    #[error("filesystem error: {0}")]
    Filesystem(String),

    #[error("path not found: {0}")]
    PathNotFound(String),

    #[error("permission denied: {0}")]
    PermissionDenied(String),

    #[error("maximum directory depth ({max}) exceeded at path: {path}")]
    MaxDepthExceeded { max: u8, path: String },

    #[error("duplicate resource name '{name}' in directory '{dir}'")]
    DuplicateName { name: String, dir: String },

    #[error("invalid name '{name}': {reason}")]
    InvalidName { name: String, reason: String },

    #[error("directory not empty: {0}")]
    NotEmpty(String),

    #[error("not a directory: {0}")]
    NotADirectory(String),

    #[error("not a file: {0}")]
    NotAFile(String),

    #[error("invalid path: {0}")]
    InvalidPath(String),

    #[error("invalid filesystem transfer: {0}")]
    InvalidMove(String),

    #[error("tree expression error: {0}")]
    TreeParse(String),

    // ------------------------------------------------------------------
    // Shell
    // ------------------------------------------------------------------
    #[error("unknown command: {0}")]
    UnknownCommand(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    // ------------------------------------------------------------------
    // Almanac command engine
    // ------------------------------------------------------------------
    #[error("almanac: {0}")]
    AlmanacParse(String),

    #[error("no prompt is awaiting a response")]
    NoPendingPrompt,

    #[error("a prompt is already awaiting a response")]
    PromptInProgress,

    #[error("command not found: {0}")]
    CommandNotFound(String),

    #[error("host process error: {0}")]
    HostProcess(String),

    // ------------------------------------------------------------------
    // Process / Scheduler
    // ------------------------------------------------------------------
    #[error("process error: {0}")]
    Process(String),

    #[error("scheduler error: {0}")]
    Scheduler(String),

    // ------------------------------------------------------------------
    // Memory
    // ------------------------------------------------------------------
    #[error("memory error: {0}")]
    Memory(String),

    #[error("out of memory: requested {requested_mb} MB, available {available_mb} MB")]
    OutOfMemory {
        requested_mb: u64,
        available_mb: u64,
    },

    // ------------------------------------------------------------------
    // Security / Authentication
    // ------------------------------------------------------------------
    #[error("authentication failed")]
    AuthenticationFailed,

    #[error("authentication is required")]
    AuthenticationRequired,

    #[error("login credentials have not been configured")]
    CredentialsNotConfigured,

    #[error("login credentials are already configured")]
    CredentialsAlreadyConfigured,

    #[error("resource authentication required for: {0}")]
    ResourceAuthenticationRequired(String),

    #[error("account locked after {attempts} failed attempts")]
    AccountLocked { attempts: u8 },

    // ------------------------------------------------------------------
    // Persistence / I/O
    // ------------------------------------------------------------------
    #[error("persistence error: {0}")]
    Persistence(String),

    #[error("persisted state is corrupt: {0}")]
    CorruptPersistence(String),

    // ------------------------------------------------------------------
    // Tauri / IPC
    // ------------------------------------------------------------------
    #[error("IPC serialization error: {0}")]
    Serialization(String),
}

/// Conversion so `AstraError` can be returned directly from Tauri commands
/// (Tauri commands require errors to implement `Into<String>` or `Serialize`).
impl serde::Serialize for AstraError {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
