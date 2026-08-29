/**
 * Process manager types — mirror `crate::process::PcbView`.
 */

import type { SchedulerAlgorithm } from "./system";

export type ProcessType =
  "SYSTEM" | "AARU_APP" | "AARU_GAME" | "HOST_APP" | "HOST_COMMAND";

export type ProcessState =
  "NEW" | "READY" | "RUNNING" | "WAITING" | "SUSPENDED" | "TERMINATED";

export type Priority = "LOW" | "NORMAL" | "HIGH" | "SYSTEM";

export interface PcbView {
  pid: number;
  parent_pid: number | null;
  name: string;
  command: string;
  process_type: ProcessType;
  state: ProcessState;
  priority: Priority;
  cpu: string;
  memory: string;
  start_time_ms: number;
  host_backed: boolean;
  host_pid: number | null;
  protected: boolean;
  /** true → metrics + scheduler state are Aaru simulations (SIMULATED label). */
  simulated: boolean;
  workload: string;
  note: string | null;
}

// ---------------------------------------------------------------------------
// Virtual CPU scheduler (Phase 6) — mirror `crate::scheduler::SchedulerSnapshot`
// ---------------------------------------------------------------------------

/** One virtual CPU core in a {@link SchedulerSnapshot}. */
export interface CoreView {
  core: number;
  /** PID currently on this core, or null when the core is idle. */
  pid: number | null;
  /** Fraction of all elapsed ticks this core has been busy (0–1). */
  utilization: number;
}

/** Per-process scheduling detail. */
export interface SchedProcessView {
  pid: number;
  state: ProcessState;
  priority: Priority;
  /** Virtual core the process is running on right now, if any. */
  core: number | null;
  cpu_ticks: number;
  /** This process's share of one whole CPU (`cpu_ticks / elapsed_ticks`). */
  cpu_share: number;
  waiting_ticks: number;
  response_ticks: number | null;
  turnaround_ticks: number | null;
  remaining_service: number;
  bursts_completed: number;
}

export interface SchedulerAverages {
  waiting: number;
  turnaround: number;
  response: number;
  completed: number;
}

export interface SchedulerSnapshot {
  algorithm: SchedulerAlgorithm;
  /** Round-Robin time slice in ticks; null for algorithms without one. */
  quantum: number | null;
  tick: number;
  context_switches: number;
  cores: CoreView[];
  ready_queue: number[];
  /** Total virtual-CPU utilization across all cores (0–1). */
  utilization: number;
  per_core_utilization: number[];
  processes: SchedProcessView[];
  averages: SchedulerAverages;
  schedulable_count: number;
}

// ---------------------------------------------------------------------------
// Simulated memory subsystem (Phase 7) — mirror `crate::memory::MemorySnapshot`
// ---------------------------------------------------------------------------

export type ReplacementPolicy = "FIFO" | "LRU";

export interface ProcessMemoryView {
  pid: number;
  pages: number;
  resident_pages: number;
  swapped_pages: number;
  resident_mb: number;
  faults: number;
}

/** One `(owner, frame count)` span of the aggregated frame bar. */
export interface FrameSpan {
  /** null → a run of free frames. */
  pid: number | null;
  frames: number;
}

/** Real Windows physical memory — shown separately, never mixed into the sim. */
export interface HostMemory {
  total_mb: number;
  used_mb: number;
  load_percent: number;
}

export interface MemorySnapshot {
  policy: ReplacementPolicy;
  page_size_mb: number;

  ram_total_mb: number;
  ram_used_mb: number;
  ram_free_mb: number;

  frames_total: number;
  frames_used: number;
  frames_free: number;

  swap_total_mb: number;
  swap_used_mb: number;
  swap_free_mb: number;

  page_faults: number;
  page_hits: number;
  swap_ins: number;
  swap_outs: number;

  processes: ProcessMemoryView[];
  frame_spans: FrameSpan[];
  host: HostMemory | null;
}
