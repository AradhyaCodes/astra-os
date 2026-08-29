/**
 * Astra OS — Shared TypeScript types
 *
 * These types mirror the Rust structs exposed via Tauri commands.
 * They represent the high-level system configuration and are intentionally
 * kept separate from application-layer types.
 */

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/** Scheduling algorithms supported by the Astra OS kernel. */
export type SchedulerAlgorithm = "RoundRobin" | "FCFS" | "Priority";

// ---------------------------------------------------------------------------
// Memory
// ---------------------------------------------------------------------------

/** Memory management policies supported by Astra OS. */
export type MemoryPolicy = "Paging" | "Swap" | "FIFOReplacement" | "LRUReplacement";

// ---------------------------------------------------------------------------
// Filesystem
// ---------------------------------------------------------------------------

/** Rules governing the virtual filesystem. */
export interface FilesystemRules {
  /** Whether filenames are case-sensitive. */
  case_sensitive: boolean;
  /** Maximum directory nesting depth (root = 0). */
  max_depth: number;
  /** Whether spaces are allowed in names. */
  allow_spaces_in_names: boolean;
  /** Whether files must have extensions. */
  files_require_extensions: boolean;
}

// ---------------------------------------------------------------------------
// Security
// ---------------------------------------------------------------------------

/** Security policy for the Astra OS single-user environment. */
export interface SecurityConfig {
  /** Only one user account is supported in v0.1. */
  single_user: boolean;
  /** Maximum consecutive failed authentication attempts before lockout. */
  max_failed_attempts: number;
}

// ---------------------------------------------------------------------------
// Kernel
// ---------------------------------------------------------------------------

/** Core hardware configuration of the virtual machine. */
export interface KernelConfig {
  /** Number of virtual CPU cores. */
  cpu_cores: number;
  /** Amount of virtual RAM in megabytes. */
  ram_mb: number;
  /** Virtual disk capacity in megabytes. */
  disk_mb: number;
  /** Maximum directory nesting depth for the virtual filesystem. */
  max_filesystem_depth: number;
}

// ---------------------------------------------------------------------------
// Top-level SystemConfig (returned by get_system_config command)
// ---------------------------------------------------------------------------

/**
 * Complete system configuration returned by the `get_system_config` Tauri command.
 * Represents the fixed hardware and policy parameters of the Astra OS v0.1 instance.
 */
export interface SystemConfig {
  kernel: KernelConfig;
  supported_schedulers: SchedulerAlgorithm[];
  memory_policies: MemoryPolicy[];
  filesystem: FilesystemRules;
  security: SecurityConfig;
  /** Kernel/OS version string. */
  version: string;
}
