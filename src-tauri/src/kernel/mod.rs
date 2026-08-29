//! Aaru-OS — Kernel module
//!
//! Defines the fixed hardware configuration and high-level kernel parameters
//! for the Aaru-OS v0.1 virtual machine.
//!
//! **Phase 0**: Data structures and constants only — no runtime behaviour.

use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Constants — Aaru-OS v0.1 fixed configuration
// ---------------------------------------------------------------------------

/// Number of virtual CPU cores.
pub const CPU_CORES: u8 = 2;

/// Virtual RAM in megabytes.
pub const RAM_MB: u32 = 4096;

/// Virtual disk capacity in megabytes (16 GB).
pub const DISK_MB: u32 = 16384;

/// Maximum directory nesting depth in the virtual filesystem.
/// Kept in step with [`crate::filesystem::validation::MAX_DEPTH`].
pub const MAX_FILESYSTEM_DEPTH: u8 = 64;

/// Maximum consecutive failed authentication attempts before lockout.
pub const MAX_FAILED_ATTEMPTS: u8 = 3;

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// Scheduling algorithms that will be implemented in Aaru-OS.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum SchedulerAlgorithm {
    /// Round Robin (default) — equal time slices across all processes.
    RoundRobin,
    /// First-Come, First-Served.
    #[serde(rename = "FCFS")]
    Fcfs,
    /// Priority scheduling — higher priority processes run first.
    Priority,
}

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/// Memory management policies planned for Aaru-OS.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub enum MemoryPolicy {
    /// Page-based virtual memory.
    Paging,
    /// Swap space for overcommitted memory.
    Swap,
    /// FIFO page replacement algorithm.
    #[serde(rename = "FIFOReplacement")]
    FifoReplacement,
    /// Least Recently Used page replacement algorithm.
    #[serde(rename = "LRUReplacement")]
    LruReplacement,
}

// ---------------------------------------------------------------------------
// Filesystem rules
// ---------------------------------------------------------------------------

/// Rules governing the virtual filesystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FilesystemRules {
    /// Whether filenames are case-sensitive.
    pub case_sensitive: bool,
    /// Maximum allowed directory nesting depth.
    pub max_depth: u8,
    /// Whether spaces are permitted in names (false for Aaru-OS).
    pub allow_spaces_in_names: bool,
    /// Whether regular files must have extensions.
    pub files_require_extensions: bool,
}

impl Default for FilesystemRules {
    fn default() -> Self {
        Self {
            case_sensitive: true,
            max_depth: MAX_FILESYSTEM_DEPTH,
            allow_spaces_in_names: true,
            files_require_extensions: true,
        }
    }
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/// Security policy for the Aaru-OS single-user environment.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    /// Whether the OS operates in single-user mode.
    pub single_user: bool,
    /// Maximum consecutive failed authentication attempts before lockout.
    pub max_failed_attempts: u8,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            single_user: true,
            max_failed_attempts: MAX_FAILED_ATTEMPTS,
        }
    }
}

// ---------------------------------------------------------------------------
// KernelConfig
// ---------------------------------------------------------------------------

/// Core hardware configuration of the Aaru-OS virtual machine.
///
/// All values are fixed for v0.1 and derived from compile-time constants.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KernelConfig {
    /// Number of virtual CPU cores.
    pub cpu_cores: u8,
    /// Virtual RAM in megabytes.
    pub ram_mb: u32,
    /// Virtual disk capacity in megabytes.
    pub disk_mb: u32,
    /// Maximum directory nesting depth.
    pub max_filesystem_depth: u8,
}

impl Default for KernelConfig {
    fn default() -> Self {
        Self {
            cpu_cores: CPU_CORES,
            ram_mb: RAM_MB,
            disk_mb: DISK_MB,
            max_filesystem_depth: MAX_FILESYSTEM_DEPTH,
        }
    }
}

// ---------------------------------------------------------------------------
// SystemConfig — top-level configuration returned via IPC
// ---------------------------------------------------------------------------

/// Complete system configuration exposed by `get_system_config`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemConfig {
    pub kernel: KernelConfig,
    pub supported_schedulers: Vec<SchedulerAlgorithm>,
    pub memory_policies: Vec<MemoryPolicy>,
    pub filesystem: FilesystemRules,
    pub security: SecurityConfig,
    /// Semantic version string for the OS.
    pub version: String,
}

impl Default for SystemConfig {
    fn default() -> Self {
        Self {
            kernel: KernelConfig::default(),
            supported_schedulers: vec![
                SchedulerAlgorithm::RoundRobin,
                SchedulerAlgorithm::Fcfs,
                SchedulerAlgorithm::Priority,
            ],
            memory_policies: vec![
                MemoryPolicy::Paging,
                MemoryPolicy::Swap,
                MemoryPolicy::FifoReplacement,
                MemoryPolicy::LruReplacement,
            ],
            filesystem: FilesystemRules::default(),
            security: SecurityConfig::default(),
            version: String::from("0.1.0"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{MemoryPolicy, SchedulerAlgorithm, SystemConfig};

    #[test]
    fn scheduler_names_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_string(&SchedulerAlgorithm::Fcfs).unwrap(),
            "\"FCFS\""
        );
    }

    #[test]
    fn memory_policy_names_match_the_frontend_contract() {
        assert_eq!(
            serde_json::to_string(&MemoryPolicy::FifoReplacement).unwrap(),
            "\"FIFOReplacement\""
        );
        assert_eq!(
            serde_json::to_string(&MemoryPolicy::LruReplacement).unwrap(),
            "\"LRUReplacement\""
        );
    }

    #[test]
    fn default_config_serializes_for_ipc() {
        let value = serde_json::to_value(SystemConfig::default()).unwrap();

        assert_eq!(value["supported_schedulers"][1], "FCFS");
        assert_eq!(value["memory_policies"][2], "FIFOReplacement");
        assert_eq!(value["memory_policies"][3], "LRUReplacement");
    }
}
