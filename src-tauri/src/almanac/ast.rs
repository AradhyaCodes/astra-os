//! The unambiguous Almanac command AST.
//!
//! Every native Almanac verb parses into one of these variants *before* any
//! filesystem work happens. Keeping an explicit AST (rather than passing raw
//! strings around) is what lets `rename a>b>c` and `open a>b>c` be handled by
//! completely different rules without the path parser ever guessing.

use crate::kernel::SchedulerAlgorithm;
use crate::memory::ReplacementPolicy;

/// Where an editor-backed command should open its file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditorTarget {
    /// `almanac write notes.txt` — no editor, just create/prepare the file.
    None,
    /// `almanac write notes.txt in VSCode` — hand off to an app-launch stub.
    App(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlmanacCommand {
    /// `almanac` with no arguments — print the native command reference.
    Help,

    // ---- Navigation ----
    Open {
        path: String,
        /// `almanac open file.js in vsc` — open the file in an editor instead
        /// of entering a directory. [`EditorTarget::None`] is a plain `cd`.
        editor: EditorTarget,
    },
    Back,
    Root,
    Scan,

    // ---- Directory creation ----
    Gen {
        path: String,
    },
    Mgen {
        expression: String,
    },

    // ---- File creation / editing ----
    Write {
        path: String,
        editor: EditorTarget,
    },
    Rewrite {
        path: String,
        editor: EditorTarget,
    },

    // ---- Delete (confirmation handled by the engine) ----
    Destroy {
        path: String,
    },

    // ---- Rename: existing resource path + new leaf name ----
    Rename {
        path: String,
        new_name: String,
    },

    // ---- Move / copy ----
    Transfer {
        from: String,
        to: String,
    },
    Copy {
        from: String,
        to: String,
    },

    // ---- Search / metadata ----
    Lookout {
        query: String,
    },
    Inspect {
        path: String,
    },

    // ---- Locking (interactive password prompts handled by the engine) ----
    Lock {
        path: String,
    },
    Unlock {
        path: String,
    },

    // ---- Host filesystem bridge ----
    /// `almanac mount` (no arg → native picker) / `almanac mount <windows path>`.
    Mount {
        path: Option<String>,
    },
    Unmount {
        alias: String,
    },
    Mounts,

    // ---- Applications & processes ----
    Run {
        application: String,
        args: Vec<String>,
    },
    /// Open a HOST resource in its default registered application.
    Reveal {
        path: String,
    },
    /// `almanac process` — list the Aaru process table.
    Process,
    Terminate {
        pid: u32,
    },
    Suspend {
        pid: u32,
    },
    Resume {
        pid: u32,
    },

    // ---- Virtual CPU scheduler (Phase 6) ----
    /// `almanac scheduler` — show virtual CPU / scheduler status.
    Scheduler,
    /// `almanac scheduler change <RR|FCFS|Priority>`.
    SchedulerChange {
        algorithm: SchedulerAlgorithm,
    },
    /// `almanac scheduler tick [n]` — advance the deterministic simulation.
    SchedulerTick {
        ticks: u64,
    },

    // ---- Simulated memory subsystem (Phase 7) ----
    /// `almanac memory` — show the simulated RAM / swap / paging status.
    Memory,
    /// `almanac memory policy <FIFO|LRU>`.
    MemorySetPolicy {
        policy: ReplacementPolicy,
    },

    // ---- Session ----
    Logout,
    KillLapsession,
    Hibernate,
    Restart,
}
