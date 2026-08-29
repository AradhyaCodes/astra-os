//! Astra OS — Virtual CPU and process scheduler (Phase 6).
//!
//! Astra OS models a **2-core virtual CPU** and schedules its own simulated
//! processes on it for teaching purposes. This is a simulation:
//!
//! * it does **not** replace or drive the real Windows scheduler;
//! * host applications keep running under Windows exactly as before;
//! * only `ASTRA_APP` and `ASTRA_GAME` processes are placed on the virtual cores.
//!   `HOST_APP` / `HOST_COMMAND` processes are observed in Task Manager but are
//!   never represented as being physically scheduled by Astra.
//!
//! The simulation advances in **deterministic ticks** ([`Scheduler::tick`]).
//! Correctness never depends on wall-clock time or on frontend render speed —
//! a caller (the Almanac `scheduler tick` verb, a Task Manager poll, or the
//! lightweight background driver started in `lib.rs`) simply asks the scheduler
//! to step. Rust owns all scheduler state.
//!
//! Algorithms live behind the [`strategy::SchedulingStrategy`] trait and are
//! implemented as independent types ([`strategy::RoundRobinScheduler`],
//! [`strategy::FcfsScheduler`], [`strategy::PriorityScheduler`]) rather than as
//! branches of one conditional.

pub mod strategy;

use crate::error::AstraError;
use crate::kernel::SchedulerAlgorithm;
use crate::process::{Pid, Priority, ProcessState, ProcessType};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use strategy::{strategy_for, ReadyEntry, SchedulingStrategy};

/// Number of virtual CPU cores, taken from the fixed kernel configuration.
pub const CORE_COUNT: usize = crate::kernel::CPU_CORES as usize;

/// Parse a user-supplied scheduler name (`RR`, `FCFS`, `Priority`, …).
///
/// Shared by the Almanac parser and the `scheduler_set_algorithm` IPC command
/// so the accepted spelling set stays in one place.
pub fn parse_algorithm(raw: &str) -> Result<SchedulerAlgorithm, AstraError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "rr" | "roundrobin" | "round-robin" | "round_robin" => Ok(SchedulerAlgorithm::RoundRobin),
        "fcfs" | "fifo" => Ok(SchedulerAlgorithm::Fcfs),
        "priority" | "prio" => Ok(SchedulerAlgorithm::Priority),
        other => Err(AstraError::AlmanacParse(format!(
            "unknown scheduler '{other}' — use RR, FCFS, or Priority"
        ))),
    }
}

/// Human-readable label for a scheduler algorithm.
pub fn algorithm_label(algorithm: SchedulerAlgorithm) -> &'static str {
    match algorithm {
        SchedulerAlgorithm::RoundRobin => "Round Robin",
        SchedulerAlgorithm::Fcfs => "FCFS",
        SchedulerAlgorithm::Priority => "Priority",
    }
}

/// Which virtual-CPU-eligible class a process belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScheduleClass {
    /// `ASTRA_APP` — interactive built-in application.
    Interactive,
    /// `ASTRA_GAME` — game loop; keeps producing work indefinitely.
    Game,
}

impl ScheduleClass {
    /// `Some` only for the two process types the virtual CPU actually schedules.
    pub fn from_process_type(process_type: ProcessType) -> Option<Self> {
        match process_type {
            ProcessType::AstraApp => Some(Self::Interactive),
            ProcessType::AstraGame => Some(Self::Game),
            _ => None,
        }
    }
}

/// Numeric urgency for priority scheduling — **lower value dispatches first**.
pub fn priority_level(priority: Priority) -> u8 {
    match priority {
        Priority::System => 0,
        Priority::High => 1,
        Priority::Normal => 2,
        Priority::Low => 3,
    }
}

/// How a simulated process behaves on the virtual CPU: a total CPU demand plus
/// an optional periodic I/O wait so processes visibly cycle
/// READY → RUNNING → WAITING → READY.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Workload {
    /// CPU ticks the process needs to finish one burst of work.
    pub service_ticks: u64,
    /// After this many CPU ticks since its last I/O, the process blocks. `0`
    /// disables I/O (the process only ever yields on quantum expiry / exit).
    pub io_every: u64,
    /// How many ticks a single I/O wait lasts.
    pub io_burst: u64,
    /// `true` — on finishing a burst the process re-enters READY instead of
    /// terminating (game loops, long-lived interactive apps).
    pub looping: bool,
}

impl Workload {
    /// The default workload profile Astra gives a freshly launched built-in.
    pub fn for_class(class: ScheduleClass) -> Self {
        match class {
            ScheduleClass::Interactive => Workload {
                service_ticks: 20,
                io_every: 5,
                io_burst: 2,
                looping: true,
            },
            ScheduleClass::Game => Workload {
                service_ticks: 40,
                io_every: 10,
                io_burst: 4,
                looping: true,
            },
        }
    }

    /// A one-shot CPU-bound workload of `ticks` with no I/O — handy for tests
    /// and for reasoning about turnaround/response deterministically.
    pub fn burst(ticks: u64) -> Self {
        Workload {
            service_ticks: ticks,
            io_every: 0,
            io_burst: 0,
            looping: false,
        }
    }
}

/// Lifecycle state the *scheduler* tracks for one process. Mirrors the subset
/// of [`ProcessState`] the virtual CPU owns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
enum RunState {
    Ready,
    Running,
    Waiting,
    Suspended,
    Terminated,
}

impl RunState {
    fn to_process_state(self) -> ProcessState {
        match self {
            RunState::Ready => ProcessState::Ready,
            RunState::Running => ProcessState::Running,
            RunState::Waiting => ProcessState::Waiting,
            RunState::Suspended => ProcessState::Suspended,
            RunState::Terminated => ProcessState::Terminated,
        }
    }
}

/// The scheduler's control block for one simulated process.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct SchedProcess {
    pid: Pid,
    #[allow(dead_code)] // retained for future per-class policy / display
    class: ScheduleClass,
    priority: Priority,
    priority_level: u8,
    state: RunState,
    workload: Workload,

    // --- live counters ---
    remaining_service: u64,
    run_since_io: u64,
    slice_used: u64,
    io_remaining: u64,
    core: Option<u8>,

    // --- metrics, all in simulation ticks ---
    arrival_tick: u64,
    enqueued_tick: u64,
    first_run_tick: Option<u64>,
    completion_tick: Option<u64>,
    cpu_ticks: u64,
    waiting_ticks: u64,
    bursts_completed: u64,
    dispatch_count: u64,
}

impl SchedProcess {
    fn new(
        pid: Pid,
        class: ScheduleClass,
        priority: Priority,
        workload: Workload,
        now: u64,
    ) -> Self {
        Self {
            pid,
            class,
            priority,
            priority_level: priority_level(priority),
            state: RunState::Ready,
            workload,
            remaining_service: workload.service_ticks.max(1),
            run_since_io: 0,
            slice_used: 0,
            io_remaining: 0,
            core: None,
            arrival_tick: now,
            enqueued_tick: now,
            first_run_tick: None,
            completion_tick: None,
            cpu_ticks: 0,
            waiting_ticks: 0,
            bursts_completed: 0,
            dispatch_count: 0,
        }
    }

    fn turnaround_ticks(&self) -> Option<u64> {
        self.completion_tick
            .map(|done| done.saturating_sub(self.arrival_tick))
    }

    fn response_ticks(&self) -> Option<u64> {
        self.first_run_tick
            .map(|first| first.saturating_sub(self.arrival_tick))
    }
}

/// Retained metrics for a process that ran to completion.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
struct CompletedRecord {
    waiting: u64,
    turnaround: u64,
    response: u64,
}

// ---------------------------------------------------------------------------
// Serializable snapshot (IPC / Almanac / Task Manager)
// ---------------------------------------------------------------------------

/// One virtual core in a [`SchedulerSnapshot`].
#[derive(Debug, Clone, Serialize)]
pub struct CoreView {
    pub core: u8,
    /// PID currently on this core, if any.
    pub pid: Option<Pid>,
    /// Fraction of all elapsed ticks this core has spent busy (`0.0`–`1.0`).
    pub utilization: f64,
}

/// Per-process scheduling detail in a [`SchedulerSnapshot`].
#[derive(Debug, Clone, Serialize)]
pub struct SchedProcessView {
    pub pid: Pid,
    pub state: ProcessState,
    pub priority: Priority,
    /// Virtual core the process is running on right now, if any.
    pub core: Option<u8>,
    pub cpu_ticks: u64,
    /// `cpu_ticks / elapsed_ticks` — this process's share of one whole CPU.
    pub cpu_share: f64,
    pub waiting_ticks: u64,
    pub response_ticks: Option<u64>,
    pub turnaround_ticks: Option<u64>,
    pub remaining_service: u64,
    pub bursts_completed: u64,
}

/// Average scheduling metrics over every process that has terminated.
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct SchedulerAverages {
    pub waiting: f64,
    pub turnaround: f64,
    pub response: f64,
    pub completed: usize,
}

/// A complete, serialisable view of the virtual CPU scheduler.
#[derive(Debug, Clone, Serialize)]
pub struct SchedulerSnapshot {
    pub algorithm: SchedulerAlgorithm,
    /// Round-Robin time slice in ticks; `null` for algorithms without one.
    pub quantum: Option<u64>,
    pub tick: u64,
    pub context_switches: u64,
    /// One entry per virtual core (`CORE_COUNT` long).
    pub cores: Vec<CoreView>,
    /// PIDs waiting in the READY queue, front first.
    pub ready_queue: Vec<Pid>,
    /// Total virtual-CPU utilization across all cores (`0.0`–`1.0`).
    pub utilization: f64,
    pub per_core_utilization: Vec<f64>,
    pub processes: Vec<SchedProcessView>,
    pub averages: SchedulerAverages,
    /// Count of live (non-terminated) scheduled processes.
    pub schedulable_count: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerRuntimeSnapshot {
    algorithm: SchedulerAlgorithm,
    tick: u64,
    table: BTreeMap<Pid, SchedProcess>,
    ready: VecDeque<Pid>,
    cores: [Option<Pid>; CORE_COUNT],
    context_switches: u64,
    busy_core_ticks: u64,
    per_core_busy: [u64; CORE_COUNT],
    completed: Vec<CompletedRecord>,
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// The virtual CPU scheduler. Holds the READY queue, the per-core assignments,
/// every tracked [`SchedProcess`], and the running metric counters.
#[derive(Debug)]
pub struct Scheduler {
    strategy: Box<dyn SchedulingStrategy>,
    tick: u64,
    table: BTreeMap<Pid, SchedProcess>,
    ready: VecDeque<Pid>,
    cores: [Option<Pid>; CORE_COUNT],
    context_switches: u64,
    busy_core_ticks: u64,
    per_core_busy: [u64; CORE_COUNT],
    completed: Vec<CompletedRecord>,
}

impl Default for Scheduler {
    fn default() -> Self {
        Self::new()
    }
}

impl Scheduler {
    /// A fresh scheduler using the default algorithm (Round Robin).
    pub fn new() -> Self {
        Self::with_algorithm(SchedulerAlgorithm::RoundRobin)
    }

    pub fn with_algorithm(algorithm: SchedulerAlgorithm) -> Self {
        Self {
            strategy: strategy_for(algorithm),
            tick: 0,
            table: BTreeMap::new(),
            ready: VecDeque::new(),
            cores: [None; CORE_COUNT],
            context_switches: 0,
            busy_core_ticks: 0,
            per_core_busy: [0; CORE_COUNT],
            completed: Vec::new(),
        }
    }

    // ------------------------------------------------------------------
    // Introspection
    // ------------------------------------------------------------------

    pub fn algorithm(&self) -> SchedulerAlgorithm {
        self.strategy.algorithm()
    }

    pub fn quantum(&self) -> Option<u64> {
        self.strategy.quantum()
    }

    pub fn current_tick(&self) -> u64 {
        self.tick
    }

    pub fn context_switches(&self) -> u64 {
        self.context_switches
    }

    pub fn runtime_snapshot(&self) -> SchedulerRuntimeSnapshot {
        SchedulerRuntimeSnapshot {
            algorithm: self.algorithm(),
            tick: self.tick,
            table: self.table.clone(),
            ready: self.ready.clone(),
            cores: self.cores,
            context_switches: self.context_switches,
            busy_core_ticks: self.busy_core_ticks,
            per_core_busy: self.per_core_busy,
            completed: self.completed.clone(),
        }
    }

    pub fn from_runtime_snapshot(snapshot: SchedulerRuntimeSnapshot) -> Self {
        Self {
            strategy: strategy_for(snapshot.algorithm),
            tick: snapshot.tick,
            table: snapshot.table,
            ready: snapshot.ready,
            cores: snapshot.cores,
            context_switches: snapshot.context_switches,
            busy_core_ticks: snapshot.busy_core_ticks,
            per_core_busy: snapshot.per_core_busy,
            completed: snapshot.completed,
        }
    }

    /// Is this PID a live (non-terminated) process the virtual CPU is tracking?
    pub fn is_tracked(&self, pid: Pid) -> bool {
        self.table
            .get(&pid)
            .is_some_and(|process| process.state != RunState::Terminated)
    }

    /// The core a PID currently occupies, if any.
    pub fn core_of(&self, pid: Pid) -> Option<u8> {
        self.table.get(&pid).and_then(|process| process.core)
    }

    /// PIDs currently occupying a virtual core (one per busy core).
    pub fn running_pids(&self) -> Vec<Pid> {
        self.cores.iter().filter_map(|slot| *slot).collect()
    }

    // ------------------------------------------------------------------
    // Lifecycle hooks (driven by the process manager via `SystemState`)
    // ------------------------------------------------------------------

    /// Admit a newly launched schedulable process: NEW → READY.
    pub fn admit(
        &mut self,
        pid: Pid,
        class: ScheduleClass,
        priority: Priority,
        workload: Workload,
    ) {
        if self.table.contains_key(&pid) {
            return;
        }
        let process = SchedProcess::new(pid, class, priority, workload, self.tick);
        self.table.insert(pid, process);
        self.ready.push_back(pid);
    }

    /// A process was suspended by the user: pull it off its core / the READY
    /// queue. A SUSPENDED process is never scheduled.
    pub fn suspend(&mut self, pid: Pid) {
        let Some(process) = self.table.get_mut(&pid) else {
            return;
        };
        if process.state == RunState::Terminated {
            return;
        }
        if let Some(core) = process.core.take() {
            self.cores[core as usize] = None;
        }
        process.slice_used = 0;
        process.state = RunState::Suspended;
        self.ready.retain(|queued| *queued != pid);
    }

    /// A process was resumed by the user: SUSPENDED → READY.
    pub fn resume(&mut self, pid: Pid) {
        let now = self.tick;
        let Some(process) = self.table.get_mut(&pid) else {
            return;
        };
        if process.state != RunState::Suspended {
            return;
        }
        process.state = RunState::Ready;
        process.enqueued_tick = now;
        process.run_since_io = 0;
        self.ready.push_back(pid);
    }

    /// A process was terminated by the user (or otherwise removed). Drop it from
    /// every queue and core immediately; it is not recorded as "completed"
    /// because it did not finish its work.
    pub fn remove(&mut self, pid: Pid) {
        let Some(process) = self.table.remove(&pid) else {
            return;
        };
        if let Some(core) = process.core {
            self.cores[core as usize] = None;
        }
        self.ready.retain(|queued| *queued != pid);
    }

    /// Switch scheduling algorithm. Everything currently on a core is placed
    /// back at the front of the READY queue (core order preserved), slice
    /// counters reset, cumulative metrics (tick, context switches, utilization)
    /// carried over.
    pub fn set_algorithm(&mut self, algorithm: SchedulerAlgorithm) {
        if algorithm == self.algorithm() {
            return;
        }
        let now = self.tick;
        let mut requeued: Vec<Pid> = Vec::new();
        for slot in self.cores.iter_mut() {
            if let Some(pid) = slot.take() {
                if let Some(process) = self.table.get_mut(&pid) {
                    process.state = RunState::Ready;
                    process.core = None;
                    process.slice_used = 0;
                    process.enqueued_tick = now;
                }
                requeued.push(pid);
            }
        }
        requeued.extend(self.ready.drain(..));
        self.ready = VecDeque::from(requeued);
        self.strategy = strategy_for(algorithm);
    }

    // ------------------------------------------------------------------
    // The deterministic simulation tick
    // ------------------------------------------------------------------

    /// Advance the simulation by exactly one tick and return every process
    /// state transition it produced, in order, as `(pid, new_state)` pairs so
    /// the caller can mirror them onto the real process table.
    ///
    /// Order within a tick:
    /// 1. terminated jobs from the previous tick are forgotten;
    /// 2. WAITING jobs age; expired I/O returns them to READY;
    /// 3. each busy core runs one tick — a job may finish, block for I/O, or
    ///    (Round-Robin) be preempted when its quantum is spent;
    /// 4. the strategy fills every idle core from the READY queue — each
    ///    dispatch is one context switch;
    /// 5. every job still in READY accrues one tick of waiting time.
    pub fn tick(&mut self) -> Vec<(Pid, ProcessState)> {
        let mut transitions: Vec<(Pid, RunState)> = Vec::new();

        // 1. Forget jobs that terminated on the previous tick.
        self.table
            .retain(|_, process| process.state != RunState::Terminated);

        self.tick += 1;
        let now = self.tick;
        let quantum = self.strategy.quantum();

        // 2. Age WAITING jobs.
        let mut woke: Vec<Pid> = Vec::new();
        for process in self.table.values_mut() {
            if process.state == RunState::Waiting {
                process.io_remaining = process.io_remaining.saturating_sub(1);
                if process.io_remaining == 0 {
                    process.state = RunState::Ready;
                    process.enqueued_tick = now;
                    process.run_since_io = 0;
                    woke.push(process.pid);
                }
            }
        }
        for pid in woke {
            self.ready.push_back(pid);
            transitions.push((pid, RunState::Ready));
        }

        // 3. Service each busy core for one tick.
        for core in 0..CORE_COUNT {
            let Some(pid) = self.cores[core] else {
                continue;
            };
            let process = self
                .table
                .get_mut(&pid)
                .expect("a core only ever holds a live PID");

            process.remaining_service = process.remaining_service.saturating_sub(1);
            process.cpu_ticks += 1;
            process.slice_used += 1;
            process.run_since_io += 1;
            self.busy_core_ticks += 1;
            self.per_core_busy[core] += 1;

            let io_due =
                process.workload.io_every > 0 && process.run_since_io >= process.workload.io_every;

            if process.remaining_service == 0 {
                process.bursts_completed += 1;
                if process.workload.looping {
                    process.remaining_service = process.workload.service_ticks.max(1);
                    process.run_since_io = 0;
                    process.slice_used = 0;
                    process.state = RunState::Ready;
                    process.enqueued_tick = now;
                    process.core = None;
                    self.cores[core] = None;
                    self.ready.push_back(pid);
                    transitions.push((pid, RunState::Ready));
                } else {
                    process.completion_tick = Some(now);
                    process.state = RunState::Terminated;
                    process.core = None;
                    self.cores[core] = None;
                    let record = CompletedRecord {
                        waiting: process.waiting_ticks,
                        turnaround: process.turnaround_ticks().unwrap_or(0),
                        response: process.response_ticks().unwrap_or(0),
                    };
                    self.completed.push(record);
                    transitions.push((pid, RunState::Terminated));
                }
            } else if io_due {
                process.state = RunState::Waiting;
                process.io_remaining = process.workload.io_burst.max(1);
                process.slice_used = 0;
                process.core = None;
                self.cores[core] = None;
                transitions.push((pid, RunState::Waiting));
            } else if quantum.is_some_and(|q| process.slice_used >= q) {
                process.state = RunState::Ready;
                process.slice_used = 0;
                process.enqueued_tick = now;
                process.core = None;
                self.cores[core] = None;
                self.ready.push_back(pid);
                transitions.push((pid, RunState::Ready));
            }
        }

        // 4. Dispatch READY jobs onto idle cores.
        for core in 0..CORE_COUNT {
            if self.cores[core].is_some() {
                continue;
            }
            let entries: Vec<ReadyEntry> = self
                .ready
                .iter()
                .map(|pid| {
                    let process = &self.table[pid];
                    ReadyEntry {
                        pid: *pid,
                        priority_level: process.priority_level,
                        enqueued_tick: process.enqueued_tick,
                    }
                })
                .collect();
            let Some(index) = self.strategy.pick(&entries) else {
                break;
            };
            let pid = self
                .ready
                .remove(index)
                .expect("strategy returned an in-range index");
            self.cores[core] = Some(pid);
            let process = self.table.get_mut(&pid).expect("READY PID is tracked");
            process.state = RunState::Running;
            process.core = Some(core as u8);
            process.slice_used = 0;
            if process.first_run_tick.is_none() {
                process.first_run_tick = Some(now);
            }
            process.dispatch_count += 1;
            self.context_switches += 1;
            transitions.push((pid, RunState::Running));
        }

        // 5. Accrue waiting time for everything still in READY.
        let ready_now: Vec<Pid> = self.ready.iter().copied().collect();
        for pid in ready_now {
            if let Some(process) = self.table.get_mut(&pid) {
                process.waiting_ticks += 1;
            }
        }

        transitions
            .into_iter()
            .map(|(pid, state)| (pid, state.to_process_state()))
            .collect()
    }

    // ------------------------------------------------------------------
    // Snapshot
    // ------------------------------------------------------------------

    fn utilization(&self) -> f64 {
        if self.tick == 0 {
            return 0.0;
        }
        self.busy_core_ticks as f64 / (CORE_COUNT as f64 * self.tick as f64)
    }

    fn core_utilization(&self, core: usize) -> f64 {
        if self.tick == 0 {
            return 0.0;
        }
        self.per_core_busy[core] as f64 / self.tick as f64
    }

    fn averages(&self) -> SchedulerAverages {
        if self.completed.is_empty() {
            return SchedulerAverages::default();
        }
        let count = self.completed.len();
        let sum = |select: fn(&CompletedRecord) -> u64| {
            self.completed.iter().map(select).sum::<u64>() as f64 / count as f64
        };
        SchedulerAverages {
            waiting: sum(|record| record.waiting),
            turnaround: sum(|record| record.turnaround),
            response: sum(|record| record.response),
            completed: count,
        }
    }

    pub fn snapshot(&self) -> SchedulerSnapshot {
        let mut processes: Vec<SchedProcessView> = self
            .table
            .values()
            .filter(|process| process.state != RunState::Terminated)
            .map(|process| SchedProcessView {
                pid: process.pid,
                state: process.state.to_process_state(),
                priority: process.priority,
                core: process.core,
                cpu_ticks: process.cpu_ticks,
                cpu_share: if self.tick == 0 {
                    0.0
                } else {
                    process.cpu_ticks as f64 / self.tick as f64
                },
                waiting_ticks: process.waiting_ticks,
                response_ticks: process.response_ticks(),
                turnaround_ticks: process.turnaround_ticks(),
                remaining_service: process.remaining_service,
                bursts_completed: process.bursts_completed,
            })
            .collect();
        processes.sort_by_key(|view| view.pid);

        let cores = (0..CORE_COUNT)
            .map(|core| CoreView {
                core: core as u8,
                pid: self.cores[core],
                utilization: self.core_utilization(core),
            })
            .collect();

        SchedulerSnapshot {
            algorithm: self.algorithm(),
            quantum: self.quantum(),
            tick: self.tick,
            context_switches: self.context_switches,
            cores,
            ready_queue: self.ready.iter().copied().collect(),
            utilization: self.utilization(),
            per_core_utilization: (0..CORE_COUNT).map(|c| self.core_utilization(c)).collect(),
            averages: self.averages(),
            schedulable_count: processes.len(),
            processes,
        }
    }
}

#[cfg(test)]
mod tests;
