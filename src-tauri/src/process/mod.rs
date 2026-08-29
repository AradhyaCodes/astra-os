//! Astra OS — Application launcher & process manager (Phase 5).
//!
//! Astra keeps its own **Process Control Block** table. Two kinds of process
//! live in it:
//!
//! * *simulated* Astra processes (`SYSTEM`, `ASTRA_APP`, `ASTRA_GAME`) — Astra owns
//!   their (simulated) state, priority and workload metrics;
//! * *host-backed* processes (`HOST_APP`, `HOST_COMMAND`) — Windows owns their
//!   real execution. Astra only **tracks/observes** them and can terminate the
//!   ones it launched. Astra never claims to drive the Windows scheduler and
//!   never fabricates exact host CPU numbers.
//!
//! The CPU scheduler itself is deliberately **not** implemented in this phase.

pub mod host_apps;
pub mod registry;

use crate::error::AstraError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::process::Child;
use std::time::{SystemTime, UNIX_EPOCH};

pub use host_apps::{list_host_apps, resolve_host_app, HostAppInfo, HostAppResolution};
pub use registry::{find_builtin, AstraAppDef, BuiltinKind};

pub type Pid = u32;

const KERNEL_PID: Pid = 1;
const ALMANAC_PID: Pid = 2;
const FIRST_DYNAMIC_PID: Pid = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProcessType {
    System,
    #[serde(alias = "AARU_APP")]
    AstraApp,
    #[serde(alias = "AARU_GAME")]
    AstraGame,
    HostApp,
    HostCommand,
}

impl ProcessType {
    fn is_host(self) -> bool {
        matches!(self, ProcessType::HostApp | ProcessType::HostCommand)
    }

    /// The two process types the Phase 6 virtual CPU actually schedules. Host
    /// processes are observed only; `System` processes are always-on services
    /// and are not placed on a virtual core.
    pub(crate) fn is_schedulable(self) -> bool {
        matches!(self, ProcessType::AstraApp | ProcessType::AstraGame)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProcessState {
    New,
    Ready,
    Running,
    Waiting,
    Suspended,
    Terminated,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Priority {
    Low,
    Normal,
    High,
    System,
}

/// A request to start an application, produced by `almanac run` (kept for the
/// stable parse boundary established in Phase 3).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApplicationRequest {
    pub application: String,
    pub args: Vec<String>,
}

impl ApplicationRequest {
    pub fn new(application: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            application: application.into(),
            args,
        }
    }
}

/// The Process Control Block.
#[derive(Debug)]
struct Pcb {
    pid: Pid,
    parent_pid: Option<Pid>,
    name: String,
    command: String,
    process_type: ProcessType,
    state: ProcessState,
    priority: Priority,
    start_time_ms: u64,
    host_backed: bool,
    host_pid: Option<u32>,
    protected: bool,
    /// Simulated workload metrics (only meaningful for simulated processes).
    sim_cpu_pct: f64,
    sim_mem_mb: f64,
    workload: String,
    note: Option<String>,
    /// Live handle for host processes Astra launched — the *only* way Astra can
    /// terminate a real process (so it can never target an arbitrary PID).
    child: Option<Child>,
    /// True only while this runtime can prove it launched the host PID. A
    /// restored hibernate record is observation-only because Windows may have
    /// reused the numeric PID while Astra was away.
    termination_authorized: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRuntimeEntry {
    pid: Pid,
    parent_pid: Option<Pid>,
    name: String,
    command: String,
    process_type: ProcessType,
    state: ProcessState,
    priority: Priority,
    start_time_ms: u64,
    host_backed: bool,
    host_pid: Option<u32>,
    protected: bool,
    sim_cpu_pct: f64,
    sim_mem_mb: f64,
    workload: String,
    note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessRuntimeSnapshot {
    next_pid: Pid,
    entries: Vec<ProcessRuntimeEntry>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LifecycleProcessSummary {
    pub running_astra: usize,
    pub running_host: usize,
    pub host_names: Vec<String>,
}

/// Serialisable view of one process for IPC / Task Manager.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PcbView {
    pub pid: Pid,
    pub parent_pid: Option<Pid>,
    pub name: String,
    pub command: String,
    pub process_type: ProcessType,
    pub state: ProcessState,
    pub priority: Priority,
    pub cpu: String,
    pub memory: String,
    pub start_time_ms: u64,
    pub host_backed: bool,
    pub host_pid: Option<u32>,
    pub protected: bool,
    /// True when the metrics and scheduling state are Astra simulations rather
    /// than real OS numbers. Drives the SIMULATED / HOST label in Task Manager.
    pub simulated: bool,
    pub workload: String,
    pub note: Option<String>,
}

impl Pcb {
    fn view(&self) -> PcbView {
        let simulated = !self.process_type.is_host();
        let (cpu, memory) = if simulated {
            (
                format!("{:.0}% (sim)", self.sim_cpu_pct),
                format!("{:.0} MB (sim)", self.sim_mem_mb),
            )
        } else {
            ("host-managed".to_string(), "host-managed".to_string())
        };
        PcbView {
            pid: self.pid,
            parent_pid: self.parent_pid,
            name: self.name.clone(),
            command: self.command.clone(),
            process_type: self.process_type,
            state: self.state,
            priority: self.priority,
            cpu,
            memory,
            start_time_ms: self.start_time_ms,
            host_backed: self.host_backed,
            host_pid: self.host_pid,
            protected: self.protected,
            simulated,
            workload: self.workload.clone(),
            note: self.note.clone(),
        }
    }
}

/// Best-effort kill of an OS PID Astra launched. Errors are swallowed — the
/// process may already be gone.
fn kill_os_pid(pid: u32) {
    let mut command = if cfg!(windows) {
        let mut c = std::process::Command::new("taskkill");
        c.args(["/PID", &pid.to_string(), "/T", "/F"]);
        c
    } else {
        let mut c = std::process::Command::new("kill");
        c.args(["-TERM", &pid.to_string()]);
        c
    };
    let _ = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

/// Outcome of a successful launch.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct LaunchReport {
    pub process: PcbView,
    /// AppId of a Tauri window the UI should open for a built-in app, if any.
    pub window: Option<String>,
}

#[derive(Debug)]
pub struct ProcessManager {
    table: BTreeMap<Pid, Pcb>,
    next_pid: Pid,
}

impl Default for ProcessManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ProcessManager {
    pub fn new() -> Self {
        let mut table = BTreeMap::new();
        table.insert(
            KERNEL_PID,
            Pcb {
                pid: KERNEL_PID,
                parent_pid: None,
                name: "astra-kernel".to_string(),
                command: "<kernel>".to_string(),
                process_type: ProcessType::System,
                state: ProcessState::Running,
                priority: Priority::System,
                start_time_ms: now_ms(),
                host_backed: false,
                host_pid: None,
                protected: true,
                sim_cpu_pct: 1.0,
                sim_mem_mb: 24.0,
                workload: "kernel services".to_string(),
                note: None,
                child: None,
                termination_authorized: false,
            },
        );
        table.insert(
            ALMANAC_PID,
            Pcb {
                pid: ALMANAC_PID,
                parent_pid: Some(KERNEL_PID),
                name: "Almanac".to_string(),
                command: "astra:almanac".to_string(),
                process_type: ProcessType::System,
                state: ProcessState::Running,
                priority: Priority::System,
                start_time_ms: now_ms(),
                host_backed: false,
                host_pid: None,
                protected: true,
                sim_cpu_pct: 2.0,
                sim_mem_mb: 40.0,
                workload: "command shell + launcher".to_string(),
                note: None,
                child: None,
                termination_authorized: false,
            },
        );
        Self {
            table,
            next_pid: FIRST_DYNAMIC_PID,
        }
    }

    /// The launcher/shell process every `almanac run` is parented to.
    pub fn launcher_pid(&self) -> Pid {
        ALMANAC_PID
    }

    fn allocate_pid(&mut self) -> Pid {
        let pid = self.next_pid;
        // Monotonic — terminated PIDs are never reused.
        self.next_pid = self.next_pid.checked_add(1).expect("pid space exhausted");
        pid
    }

    // ------------------------------------------------------------------
    // Launching
    // ------------------------------------------------------------------

    /// Register a simulated built-in application or game.
    pub fn spawn_builtin(&mut self, def: &AstraAppDef, parent: Pid) -> LaunchReport {
        let pid = self.allocate_pid();
        let process_type = match def.kind {
            BuiltinKind::Game => ProcessType::AstraGame,
            BuiltinKind::App => ProcessType::AstraApp,
        };
        let pcb = Pcb {
            pid,
            parent_pid: Some(parent),
            name: def.name.to_string(),
            command: format!("astra:{}", def.key.to_lowercase()),
            process_type,
            // Enters the virtual CPU's READY queue on launch; the Phase 6
            // scheduler promotes it to RUNNING when a core is free.
            state: ProcessState::Ready,
            priority: def.priority,
            start_time_ms: now_ms(),
            host_backed: false,
            host_pid: None,
            protected: false,
            sim_cpu_pct: def.sim_cpu_pct,
            sim_mem_mb: def.sim_mem_mb,
            workload: def.workload.to_string(),
            note: None,
            child: None,
            termination_authorized: false,
        };
        let view = pcb.view();
        self.table.insert(pid, pcb);
        LaunchReport {
            process: view,
            window: def.window.map(str::to_string),
        }
    }

    /// Register a host-backed process Astra just spawned.
    ///
    /// `child` is `Some` when Astra owns the `std::process::Child` handle (an
    /// app launched via `almanac run`); it is `None` for a streamed host-shell
    /// command, where Astra only recorded the OS PID at spawn time. Either way
    /// the PID was captured by Astra at launch, so termination can never target
    /// a process Astra did not start.
    #[allow(clippy::too_many_arguments)] // a PCB constructor, not a code smell
    pub fn register_host(
        &mut self,
        name: impl Into<String>,
        command: impl Into<String>,
        process_type: ProcessType,
        parent: Pid,
        child: Option<Child>,
        host_pid: Option<u32>,
        note: Option<String>,
    ) -> PcbView {
        debug_assert!(process_type.is_host());
        let pid = self.allocate_pid();
        let host_pid = child.as_ref().map(Child::id).or(host_pid);
        let pcb = Pcb {
            pid,
            parent_pid: Some(parent),
            name: name.into(),
            command: command.into(),
            process_type,
            state: ProcessState::Running,
            priority: Priority::Normal,
            start_time_ms: now_ms(),
            host_backed: true,
            host_pid,
            protected: false,
            sim_cpu_pct: 0.0,
            sim_mem_mb: 0.0,
            workload: "host-managed".to_string(),
            note,
            child,
            termination_authorized: true,
        };
        let view = pcb.view();
        self.table.insert(pid, pcb);
        view
    }

    /// Mark a host-shell command (registered PID-only) as terminated when its
    /// streaming task reports the OS process exited.
    pub fn mark_exited(&mut self, pid: Pid) {
        if let Some(pcb) = self.table.get_mut(&pid) {
            if pcb.state != ProcessState::Terminated {
                pcb.state = ProcessState::Terminated;
                pcb.child = None;
            }
        }
    }

    // ------------------------------------------------------------------
    // Lifecycle
    // ------------------------------------------------------------------

    fn pcb_mut(&mut self, pid: Pid) -> Result<&mut Pcb, AstraError> {
        self.table.get_mut(&pid).ok_or_else(|| {
            AstraError::Process(format!(
                "no Astra process with PID {pid} — Astra only manages processes it launched or tracks"
            ))
        })
    }

    pub fn terminate(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        {
            let pcb = self.pcb_mut(pid)?;
            if pcb.protected {
                return Err(AstraError::PermissionDenied(format!(
                    "PID {pid} ({}) is a protected system process",
                    pcb.name
                )));
            }
            if pcb.state == ProcessState::Terminated {
                return Err(AstraError::Process(format!(
                    "PID {pid} has already terminated"
                )));
            }
            if let Some(child) = pcb.child.as_mut() {
                // Astra holds this Child, so it can only ever kill a process it
                // started itself.
                let _ = child.kill();
                let _ = child.wait();
            } else if pcb.host_backed && pcb.termination_authorized {
                if let Some(os_pid) = pcb.host_pid {
                    // PID-only host-shell command: kill the tree Astra launched,
                    // at the current user's privilege (never elevated).
                    kill_os_pid(os_pid);
                }
            }
            pcb.state = ProcessState::Terminated;
            pcb.child = None;
        }
        Ok(self.table[&pid].view())
    }

    pub fn suspend(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        let pcb = self.pcb_mut(pid)?;
        if pcb.protected {
            return Err(AstraError::PermissionDenied(format!(
                "PID {pid} ({}) is a protected system process",
                pcb.name
            )));
        }
        if pcb.host_backed {
            return Err(AstraError::Process(format!(
                "PID {pid} is host-backed — Windows controls its execution and Astra cannot \
                 suspend it"
            )));
        }
        match pcb.state {
            ProcessState::Running | ProcessState::Ready => {
                pcb.state = ProcessState::Suspended;
                Ok(pcb.view())
            }
            other => Err(AstraError::Process(format!(
                "cannot suspend PID {pid} from state {other:?}"
            ))),
        }
    }

    pub fn resume(&mut self, pid: Pid) -> Result<PcbView, AstraError> {
        let pcb = self.pcb_mut(pid)?;
        if pcb.host_backed {
            return Err(AstraError::Process(format!(
                "PID {pid} is host-backed — Astra never suspended it"
            )));
        }
        match pcb.state {
            ProcessState::Suspended => {
                // Back into the scheduler's READY queue, not straight onto a
                // core — the virtual CPU decides when it next runs.
                pcb.state = ProcessState::Ready;
                Ok(pcb.view())
            }
            other => Err(AstraError::Process(format!(
                "cannot resume PID {pid} from state {other:?}"
            ))),
        }
    }

    /// Apply a state transition decided by the Phase 6 virtual CPU scheduler.
    /// Only ever touches schedulable Astra processes, and never resurrects one
    /// the user already terminated or overrides a user suspend.
    pub(crate) fn set_state_from_scheduler(&mut self, pid: Pid, state: ProcessState) {
        if let Some(pcb) = self.table.get_mut(&pid) {
            if pcb.process_type.is_schedulable()
                && pcb.state != ProcessState::Terminated
                && pcb.state != ProcessState::Suspended
            {
                pcb.state = state;
            }
        }
    }

    // ------------------------------------------------------------------
    // Observation
    // ------------------------------------------------------------------

    /// Reconcile host-backed processes with reality: if the OS process has
    /// exited, mark the PCB `TERMINATED`.
    pub fn reap(&mut self) {
        for pcb in self.table.values_mut() {
            if pcb.state == ProcessState::Terminated {
                continue;
            }
            if let Some(child) = pcb.child.as_mut() {
                if matches!(child.try_wait(), Ok(Some(_))) {
                    pcb.state = ProcessState::Terminated;
                    pcb.child = None;
                }
            }
        }
    }

    pub fn list(&mut self) -> Vec<PcbView> {
        self.reap();
        self.table.values().map(Pcb::view).collect()
    }

    pub fn get(&self, pid: Pid) -> Option<PcbView> {
        self.table.get(&pid).map(Pcb::view)
    }

    pub fn tracked_pids(&self) -> Vec<Pid> {
        self.table.keys().copied().collect()
    }

    pub fn runtime_snapshot(&self) -> ProcessRuntimeSnapshot {
        ProcessRuntimeSnapshot {
            next_pid: self.next_pid,
            entries: self
                .table
                .values()
                .map(|pcb| ProcessRuntimeEntry {
                    pid: pcb.pid,
                    parent_pid: pcb.parent_pid,
                    name: pcb.name.clone(),
                    command: pcb.command.clone(),
                    process_type: pcb.process_type,
                    state: pcb.state,
                    priority: pcb.priority,
                    start_time_ms: pcb.start_time_ms,
                    host_backed: pcb.host_backed,
                    host_pid: pcb.host_pid,
                    protected: pcb.protected,
                    sim_cpu_pct: pcb.sim_cpu_pct,
                    sim_mem_mb: pcb.sim_mem_mb,
                    workload: pcb.workload.clone(),
                    note: pcb.note.clone(),
                })
                .collect(),
        }
    }

    pub fn from_runtime_snapshot(snapshot: ProcessRuntimeSnapshot) -> Self {
        let mut table = BTreeMap::new();
        for entry in snapshot.entries {
            let host_alive = entry.host_pid.is_some_and(host_pid_exists);
            let restored_state = if entry.host_backed && !host_alive {
                ProcessState::Terminated
            } else {
                entry.state
            };
            let restored_note = if entry.host_backed {
                Some(if host_alive {
                    "restored from hibernate; host process observed but not termination-authorized"
                        .to_string()
                } else {
                    "host process no longer exists after hibernate".to_string()
                })
            } else {
                entry.note
            };
            table.insert(
                entry.pid,
                Pcb {
                    pid: entry.pid,
                    parent_pid: entry.parent_pid,
                    name: entry.name,
                    command: entry.command,
                    process_type: entry.process_type,
                    state: restored_state,
                    priority: entry.priority,
                    start_time_ms: entry.start_time_ms,
                    host_backed: entry.host_backed,
                    host_pid: entry.host_pid,
                    protected: entry.protected,
                    sim_cpu_pct: entry.sim_cpu_pct,
                    sim_mem_mb: entry.sim_mem_mb,
                    workload: entry.workload,
                    note: restored_note,
                    child: None,
                    termination_authorized: false,
                },
            );
        }
        Self {
            table,
            next_pid: snapshot.next_pid.max(FIRST_DYNAMIC_PID),
        }
    }

    pub fn lifecycle_summary(&mut self) -> LifecycleProcessSummary {
        self.reap();
        let mut host_names = Vec::new();
        let mut running_astra = 0;
        let mut running_host = 0;
        for pcb in self.table.values() {
            if pcb.protected || pcb.state == ProcessState::Terminated {
                continue;
            }
            if pcb.host_backed {
                running_host += 1;
                host_names.push(pcb.name.clone());
            } else {
                running_astra += 1;
            }
        }
        LifecycleProcessSummary {
            running_astra,
            running_host,
            host_names,
        }
    }

    /// Terminate only dynamic processes that this Astra runtime can prove it
    /// owns. Protected services and observation-only restored host PIDs are
    /// never targeted.
    pub fn shutdown_managed(&mut self) {
        let pids: Vec<Pid> = self
            .table
            .values()
            .filter(|pcb| !pcb.protected && pcb.state != ProcessState::Terminated)
            .map(|pcb| pcb.pid)
            .collect();
        for pid in pids {
            let _ = self.terminate(pid);
        }
    }
}

fn host_pid_exists(pid: u32) -> bool {
    if cfg!(windows) {
        std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
            .output()
            .ok()
            .is_some_and(|output| {
                String::from_utf8_lossy(&output.stdout).contains(&format!("\"{pid}\""))
            })
    } else {
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .is_ok_and(|status| status.success())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spawn_test_child() -> Child {
        let mut command = if cfg!(windows) {
            let mut c = std::process::Command::new("cmd");
            c.args(["/C", "ping -n 30 127.0.0.1 >NUL"]);
            c
        } else {
            let mut c = std::process::Command::new("sh");
            c.args(["-c", "sleep 30"]);
            c
        };
        command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[test]
    fn legacy_process_type_names_deserialize_after_the_rename() {
        assert_eq!(
            serde_json::from_str::<ProcessType>("\"AARU_APP\"").unwrap(),
            ProcessType::AstraApp
        );
        assert_eq!(
            serde_json::from_str::<ProcessType>("\"AARU_GAME\"").unwrap(),
            ProcessType::AstraGame
        );
        assert_eq!(
            serde_json::to_string(&ProcessType::AstraApp).unwrap(),
            "\"ASTRA_APP\""
        );
    }

    #[test]
    fn pids_are_unique_and_never_reused() {
        let mut manager = ProcessManager::new();
        let calc = find_builtin("calculator").unwrap();
        let a = manager.spawn_builtin(calc, ALMANAC_PID).process.pid;
        let b = manager.spawn_builtin(calc, ALMANAC_PID).process.pid;
        manager.terminate(a).unwrap();
        let c = manager.spawn_builtin(calc, ALMANAC_PID).process.pid;
        let mut seen = std::collections::BTreeSet::new();
        for pid in [KERNEL_PID, ALMANAC_PID, a, b, c] {
            assert!(seen.insert(pid), "duplicate pid {pid}");
        }
        assert!(c > b && b > a);
    }

    #[test]
    fn builtin_launch_links_to_the_launcher_process() {
        let mut manager = ProcessManager::new();
        let report = manager.spawn_builtin(find_builtin("Snake").unwrap(), manager.launcher_pid());
        assert_eq!(report.process.parent_pid, Some(ALMANAC_PID));
        assert_eq!(report.process.process_type, ProcessType::AstraGame);
        assert!(report.process.simulated);
        assert!(manager.get(ALMANAC_PID).is_some());
    }

    #[test]
    fn suspend_resume_transitions_for_a_simulated_process() {
        let mut manager = ProcessManager::new();
        let pid = manager
            .spawn_builtin(find_builtin("Tetris").unwrap(), ALMANAC_PID)
            .process
            .pid;
        assert_eq!(manager.suspend(pid).unwrap().state, ProcessState::Suspended);
        assert!(manager.suspend(pid).is_err()); // already suspended
                                                // Resume returns the process to READY (the scheduler owns RUNNING).
        assert_eq!(manager.resume(pid).unwrap().state, ProcessState::Ready);
        assert!(manager.resume(pid).is_err()); // already ready, not suspended
    }

    #[test]
    fn protected_processes_cannot_be_killed_or_suspended() {
        let mut manager = ProcessManager::new();
        assert!(matches!(
            manager.terminate(KERNEL_PID),
            Err(AstraError::PermissionDenied(_))
        ));
        assert!(matches!(
            manager.suspend(ALMANAC_PID),
            Err(AstraError::PermissionDenied(_))
        ));
    }

    #[test]
    fn unknown_pid_is_rejected() {
        let mut manager = ProcessManager::new();
        assert!(matches!(
            manager.terminate(9999),
            Err(AstraError::Process(_))
        ));
    }

    #[test]
    fn tracked_host_process_terminates_and_is_reaped() {
        let mut manager = ProcessManager::new();
        let view = manager.register_host(
            "sleeper",
            "cmd /C pause",
            ProcessType::HostCommand,
            ALMANAC_PID,
            Some(spawn_test_child()),
            None,
            None,
        );
        assert!(view.host_backed);
        assert!(view.host_pid.is_some());
        assert!(!view.simulated);
        assert!(manager.suspend(view.pid).is_err()); // host-backed cannot be suspended
        let terminated = manager.terminate(view.pid).unwrap();
        assert_eq!(terminated.state, ProcessState::Terminated);
    }

    #[test]
    fn restored_host_pid_is_observation_only_and_never_terminated() {
        let mut child = spawn_test_child();
        let os_pid = child.id();
        let mut manager = ProcessManager::new();
        manager.register_host(
            "external-test",
            "external-test",
            ProcessType::HostCommand,
            manager.launcher_pid(),
            None,
            Some(os_pid),
            None,
        );
        let snapshot = manager.runtime_snapshot();
        let mut restored = ProcessManager::from_runtime_snapshot(snapshot);
        restored.shutdown_managed();

        assert!(child.try_wait().unwrap().is_none());
        let _ = child.kill();
        let _ = child.wait();
    }
}
