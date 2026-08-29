//! Executes a parsed [`AlmanacCommand`] against [`SystemState`].
//!
//! Resolution order for a raw line:
//! 1. If the line addresses the Almanac engine (`almanac …`) it is parsed and
//!    a native command runs — these always win over any host equivalent.
//! 2. Otherwise the line is handed to the controlled host-shell layer.
//!
//! Interactive commands (`destroy`, `lock`, `unlock`, `logout`) park a
//! [`PendingPrompt`] on the session and return a [`PromptRequest`]; the UI
//! collects the (possibly masked) input and calls back through
//! [`respond`]. Passwords therefore never travel on the command line and never
//! reach command history.

use super::ast::{AlmanacCommand, EditorTarget};
use super::lexer::is_almanac_line;
use super::outcome::{AlmanacOutcome, AppLaunch, PromptRequest, StatusTag, SystemAction};
use super::parser::parse_line;
use crate::error::AaruError;
use crate::filesystem::ResourceType;
use crate::fs_provider::{AaruLocation, EntryView, ProviderKind};
use crate::process::PcbView;
use crate::shell::{tokenize_host_line, HostCommand};
use crate::state::{PendingPrompt, RunReport, SystemState};
use std::path::Path;
use zeroize::Zeroizing;

/// Native verbs, used by the parser reference output and by tab completion.
pub const NATIVE_VERBS: &[&str] = &[
    "open",
    "back",
    "root",
    "scan",
    "gen",
    "mgen",
    "write",
    "rewrite",
    "destroy",
    "rename",
    "transfer",
    "copy",
    "lookout",
    "inspect",
    "lock",
    "unlock",
    "run",
    "reveal",
    "process",
    "terminate",
    "suspend",
    "resume",
    "scheduler",
    "memory",
    "mount",
    "unmount",
    "mounts",
    "logout",
    "kill",
    "hibernate",
    "restart",
];

/// Path-taking verbs whose final argument tab completion should resolve.
pub const PATH_VERBS: &[&str] = &[
    "open", "gen", "mgen", "write", "rewrite", "destroy", "rename", "transfer", "copy", "inspect",
    "lock", "unlock", "reveal",
];

/// Evaluate one raw terminal line.
pub fn evaluate(state: &mut SystemState, cwd: &str, line: &str) -> AlmanacOutcome {
    if state.has_pending_prompt() {
        return AlmanacOutcome::line(
            StatusTag::Error,
            "a prompt is still open — answer it or press Esc to cancel",
        );
    }

    let trimmed = line.trim();
    if trimmed.is_empty() {
        return AlmanacOutcome::default();
    }

    if let Some((application, fallback_url)) = direct_app_shortcut(trimmed) {
        launch_direct_app(state, cwd, application, fallback_url)
    } else if is_almanac_line(trimmed) {
        match parse_line(trimmed) {
            Ok(command) => run_command(state, cwd, command),
            Err(error) => AlmanacOutcome::from_error(&error),
        }
    } else {
        host_fallback(state, trimmed)
    }
}

fn direct_app_shortcut(line: &str) -> Option<(&'static str, Option<&'static str>)> {
    match line.to_ascii_lowercase().as_str() {
        "chatgpt" => Some(("ChatGPT", Some("https://chatgpt.com"))),
        "claude" => Some(("Claude", Some("https://claude.ai"))),
        "brave" => Some(("Brave", None)),
        "chrome" => Some(("Chrome", None)),
        "google" => Some(("Chrome", Some("https://www.google.com"))),
        "vsc" => Some(("VSCode", None)),
        "antigravity" => Some(("Antigravity", None)),
        _ => None,
    }
}

fn launch_direct_app(
    state: &mut SystemState,
    cwd: &str,
    application: &str,
    fallback_url: Option<&str>,
) -> AlmanacOutcome {
    match launch_application(state, cwd, application, &[]) {
        Ok(outcome) => outcome,
        Err(AaruError::CommandNotFound(_)) if fallback_url.is_some() => {
            let url = fallback_url.expect("guarded above");
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Process,
                format!("{application} desktop app not found — opening {url}"),
            );
            outcome.push(
                StatusTag::Info,
                "handed to the default Windows browser — not tracked as an Aaru process",
            );
            outcome.launch = Some(AppLaunch {
                app: "$url".to_string(),
                path: Some(url.to_string()),
                args: Vec::new(),
            });
            outcome
        }
        Err(error) => AlmanacOutcome::from_error(&error),
    }
}

/// Handle the reply to an outstanding [`PromptRequest`].
pub fn respond(state: &mut SystemState, response: &str) -> AlmanacOutcome {
    respond_inner(state, response).unwrap_or_else(|error| AlmanacOutcome::from_error(&error))
}

/// Abandon any outstanding prompt (Esc in the UI).
pub fn cancel(state: &mut SystemState) -> AlmanacOutcome {
    match state.take_pending_prompt() {
        PendingPrompt::None => AlmanacOutcome::line(StatusTag::Info, "no prompt to cancel"),
        _ => AlmanacOutcome::line(StatusTag::Info, "prompt cancelled"),
    }
}

fn host_fallback(state: &mut SystemState, line: &str) -> AlmanacOutcome {
    if let Err(error) = state.require_authentication() {
        return AlmanacOutcome::from_error(&error);
    }
    let tokens = tokenize_host_line(line);
    let Some((program, args)) = tokens.split_first() else {
        return AlmanacOutcome::default();
    };
    let mut outcome = AlmanacOutcome::default();
    outcome.push(
        StatusTag::Process,
        format!("host → {}", render_command(program, args)),
    );
    outcome.shell = Some(HostCommand::new(program.clone(), args.to_vec()));
    outcome
}

fn render_command(program: &str, args: &[String]) -> String {
    if args.is_empty() {
        program.to_string()
    } else {
        format!("{program} {}", args.join(" "))
    }
}

fn run_command(state: &mut SystemState, cwd: &str, command: AlmanacCommand) -> AlmanacOutcome {
    run_command_inner(state, cwd, command)
        .unwrap_or_else(|error| AlmanacOutcome::from_error(&error))
}

fn launch_application(
    state: &mut SystemState,
    cwd: &str,
    application: &str,
    args: &[String],
) -> Result<AlmanacOutcome, AaruError> {
    Ok(match state.run_application(cwd, application, args)? {
        RunReport::HostApp { process } => {
            let mut outcome = pcb_line("launched real", &process);
            outcome.push(
                StatusTag::Info,
                "Windows controls this process's execution; Aaru only tracks its state",
            );
            outcome
        }
        RunReport::HostOpen { display, real } => {
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Process,
                format!(
                    "launched {display} via Windows — not tracked (Windows owns the process, e.g. \
                     a shortcut chains to its own launcher)"
                ),
            );
            outcome.launch = Some(AppLaunch {
                app: "$default".to_string(),
                path: Some(real),
                args: Vec::new(),
            });
            outcome
        }
        RunReport::HostLaunched { display } => {
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Process,
                format!("launched {display} via Windows (Microsoft Store app)"),
            );
            outcome.push(
                StatusTag::Info,
                "Windows owns this process — Aaru does not track it",
            );
            outcome
        }
        RunReport::Builtin { process, window } => {
            let mut outcome = pcb_line("started", &process);
            outcome.push(
                StatusTag::Info,
                format!("simulated workload: {}", process.workload),
            );
            outcome.open_window_title = window.as_ref().map(|_| process.name.clone());
            outcome.open_window = window;
            outcome
        }
    })
}

// ---------------------------------------------------------------------------
// Small helpers shared by the virtual and host branches
// ---------------------------------------------------------------------------

/// Move the current directory to `display`, virtual or host.
fn moved(display: impl Into<String>) -> AlmanacOutcome {
    let display = display.into();
    let mut outcome = AlmanacOutcome::line(StatusTag::Ok, format!("moved to {display}"));
    outcome.new_cwd = Some(display);
    outcome
}

/// One-line `[OK]` result carrying a resource path.
fn touched(verb: &str, display: &str) -> AlmanacOutcome {
    AlmanacOutcome::line(StatusTag::Ok, format!("{verb} {display}"))
}

/// Detailed metadata lines for `inspect` (host or virtual).
fn inspect_lines(view: &EntryView) -> AlmanacOutcome {
    let mut outcome = AlmanacOutcome::default();
    outcome.push(StatusTag::Ok, format!("inspect {}", view.display_path));
    outcome.push(
        StatusTag::Info,
        format!(
            "source: {}",
            match view.kind {
                ProviderKind::Virtual => "AARU virtual filesystem",
                ProviderKind::Host => "Windows host filesystem",
            }
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!("type: {}", if view.is_dir { "directory" } else { "file" }),
    );
    outcome.push(StatusTag::Info, format!("size: {} byte(s)", view.size));
    if let Some(ms) = view.modified_ms {
        outcome.push(StatusTag::Info, format!("modified: {ms} ms"));
    }
    if let Some(ms) = view.created_ms {
        outcome.push(StatusTag::Info, format!("created: {ms} ms"));
    }
    outcome.push(StatusTag::Info, format!("read-only: {}", view.read_only));
    outcome.push(
        StatusTag::Info,
        format!("Aaru lock: {}", if view.aaru_locked { "yes" } else { "no" }),
    );
    if let Some(real) = &view.host_real_path {
        outcome.push(StatusTag::Info, format!("host path: {real}"));
    }
    outcome
}

fn lock_prompt(id: String, kind: &str, message: &str, header: &str) -> AlmanacOutcome {
    let mut outcome = AlmanacOutcome::line(StatusTag::Auth, header.to_string());
    outcome.prompt = Some(PromptRequest {
        id,
        kind: kind.to_string(),
        message: message.to_string(),
        masked: true,
    });
    outcome
}

fn host_display(mount: &str, relative: &[String]) -> String {
    if relative.is_empty() {
        format!("HOST>{mount}")
    } else {
        format!("HOST>{mount}>{}", relative.join(">"))
    }
}

fn back_target(state: &SystemState, cwd: &str) -> Result<AlmanacOutcome, AaruError> {
    match state.route(cwd, ".")? {
        AaruLocation::Virtual(_) => {
            let parent = state.parent_directory(cwd, ".")?;
            Ok(moved(parent.path))
        }
        AaruLocation::HostRoot => Ok(moved("ROOT")),
        AaruLocation::Host { mount, relative } => {
            if relative.is_empty() {
                return Ok(moved("HOST"));
            }
            let parent = &relative[..relative.len() - 1];
            state.host_open(&mount, parent)?;
            Ok(moved(host_display(&mount, parent)))
        }
    }
}

fn scan_current(state: &SystemState, cwd: &str) -> Result<AlmanacOutcome, AaruError> {
    match state.route(cwd, ".")? {
        AaruLocation::Virtual(canonical) => {
            let entries = state.list_directory("ROOT", &canonical)?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(
                StatusTag::Ok,
                format!("AARU>{canonical} — {} item(s)", entries.len()),
            );
            for entry in entries {
                let kind = match entry.metadata.resource_type {
                    ResourceType::Directory => "dir",
                    ResourceType::File => "file",
                };
                let lock = if entry.metadata.locked {
                    " · locked"
                } else {
                    ""
                };
                outcome.push(
                    StatusTag::Info,
                    format!("{}  ({kind}{lock})", entry.metadata.name),
                );
            }
            Ok(outcome)
        }
        AaruLocation::HostRoot => {
            let mounts = state.host_mount_list()?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(StatusTag::Ok, format!("HOST — {} mount(s)", mounts.len()));
            for mount in mounts {
                outcome.push(
                    StatusTag::Info,
                    format!(
                        "{}  (mount{}{})",
                        mount.alias,
                        if mount.is_default { ", default" } else { "" },
                        if mount.available { "" } else { ", missing" }
                    ),
                );
            }
            Ok(outcome)
        }
        AaruLocation::Host { mount, relative } => {
            let entries = state.host_list(&mount, &relative)?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(
                StatusTag::Ok,
                format!(
                    "{} — {} item(s)",
                    host_display(&mount, &relative),
                    entries.len()
                ),
            );
            for entry in entries {
                let kind = if entry.is_dir { "dir" } else { "file" };
                let lock = if entry.aaru_locked {
                    " · aaru-locked"
                } else {
                    ""
                };
                outcome.push(StatusTag::Info, format!("{}  ({kind}{lock})", entry.name));
            }
            Ok(outcome)
        }
    }
}

fn relocate(
    state: &mut SystemState,
    cwd: &str,
    from: &str,
    to: &str,
    copy: bool,
) -> Result<AlmanacOutcome, AaruError> {
    let verb = if copy { "copied to" } else { "moved to" };
    match (state.route(cwd, from)?, state.route(cwd, to)?) {
        (AaruLocation::Virtual(source), AaruLocation::Virtual(target)) => {
            let info = if copy {
                state.copy_resource("ROOT", &source, &target)?
            } else {
                state.move_resource("ROOT", &source, &target)?
            };
            Ok(touched(verb, &info.path))
        }
        (
            AaruLocation::Host {
                mount: from_mount,
                relative: from_rel,
            },
            AaruLocation::Host {
                mount: to_mount,
                relative: to_rel,
            },
        ) => {
            let view = state.host_relocate(&from_mount, &from_rel, &to_mount, &to_rel, copy)?;
            Ok(touched(verb, &view.display_path))
        }

        // ---- cross-boundary: HOST → AARU ----
        (
            AaruLocation::Host {
                mount: from_mount,
                relative: from_rel,
            },
            AaruLocation::Virtual(target),
        ) => {
            if from_rel.is_empty() {
                return Err(AaruError::InvalidMove(
                    "pick a file or folder inside the mount, not the HOST>… root itself"
                        .to_string(),
                ));
            }
            let summary = state.import_host_into_virtual(&from_mount, &from_rel, &target)?;
            let mut outcome = touched(verb, &summary.created_path);
            outcome.push(
                StatusTag::Info,
                format!(
                    "copied from Windows into the Aaru filesystem — {} file(s), {} folder(s)",
                    summary.files, summary.dirs
                ),
            );
            report_skips(&mut outcome, &summary);
            if !copy {
                if summary.skipped == 0 {
                    let removed = state.host_recycle(&from_mount, &from_rel)?;
                    outcome.push(
                        StatusTag::Info,
                        format!(
                            "source sent to the Windows Recycle Bin — {} file(s), {} folder(s)",
                            removed.files, removed.folders
                        ),
                    );
                } else {
                    outcome.push(
                        StatusTag::Error,
                        "source left in place because some entries could not be copied",
                    );
                }
            }
            Ok(outcome)
        }

        // ---- cross-boundary: AARU → HOST ----
        (
            AaruLocation::Virtual(source),
            AaruLocation::Host {
                mount: to_mount,
                relative: to_rel,
            },
        ) => {
            let summary = state.export_virtual_to_host(&source, &to_mount, &to_rel)?;
            let mut outcome = touched(verb, &summary.created_path);
            outcome.push(
                StatusTag::Info,
                format!(
                    "copied from the Aaru filesystem onto Windows — {} file(s), {} folder(s)",
                    summary.files, summary.dirs
                ),
            );
            report_skips(&mut outcome, &summary);
            if !copy {
                if summary.skipped == 0 {
                    let removed = state.delete_recursive("ROOT", &source)?;
                    outcome.push(
                        StatusTag::Info,
                        format!(
                            "source removed from the Aaru filesystem — {} resource(s)",
                            removed.total_resources
                        ),
                    );
                } else {
                    outcome.push(
                        StatusTag::Error,
                        "source left in place because some entries could not be copied",
                    );
                }
            }
            Ok(outcome)
        }

        _ => Err(AaruError::InvalidMove(
            "transfer/copy needs a file or folder on each side — the bare HOST and ROOT roots are \
             not valid endpoints (pick a mount, e.g. HOST>Documents)"
                .to_string(),
        )),
    }
}

/// Append a summary of anything a cross-boundary walk had to skip.
fn report_skips(outcome: &mut AlmanacOutcome, summary: &crate::state::CrossCopySummary) {
    if summary.skipped == 0 {
        return;
    }
    outcome.push(
        StatusTag::Error,
        format!("{} entr(y/ies) skipped:", summary.skipped),
    );
    for line in &summary.skip_details {
        outcome.push(StatusTag::Info, format!("  {line}"));
    }
    let shown = summary.skip_details.len();
    if summary.skipped > shown {
        outcome.push(
            StatusTag::Info,
            format!("  …and {} more", summary.skipped - shown),
        );
    }
}

/// `[PROCESS] <action> <name> — Aaru PID N [SIM|HOST] (host PID M)`.
fn pcb_line(action: &str, view: &PcbView) -> AlmanacOutcome {
    let label = if view.simulated { "SIM" } else { "HOST" };
    let host_pid = view
        .host_pid
        .map(|pid| format!(", host PID {pid}"))
        .unwrap_or_default();
    AlmanacOutcome::line(
        StatusTag::Process,
        format!(
            "{action} {} — Aaru PID {} [{label}]{host_pid}",
            view.name, view.pid
        ),
    )
}

fn format_process_row(view: &PcbView) -> String {
    let label = if view.simulated { "SIM " } else { "HOST" };
    let parent = view
        .parent_pid
        .map(|pid| pid.to_string())
        .unwrap_or_else(|| "-".to_string());
    let note = view
        .note
        .as_ref()
        .map(|note| format!("  · {note}"))
        .unwrap_or_default();
    format!(
        "PID {:>3} (ppid {:>3})  {:<12} {:<11} {:<10} {:<7} cpu {:<11} mem {:<12} [{label}]{note}",
        view.pid,
        parent,
        format!("{:?}", view.process_type),
        format!("{:?}", view.state),
        view.name,
        format!("{:?}", view.priority),
        view.cpu,
        view.memory,
    )
}

/// `PID <n> <name>` for a scheduled process, or just `PID <n>` if unknown.
fn process_label(state: &SystemState, pid: crate::process::Pid) -> String {
    match state.process_get(pid) {
        Some(view) => format!("PID {pid} {}", view.name),
        None => format!("PID {pid}"),
    }
}

/// Render `almanac scheduler` — the virtual CPU / scheduler status block.
fn scheduler_status_lines(state: &SystemState) -> Result<AlmanacOutcome, AaruError> {
    let snapshot = state.scheduler_snapshot()?;
    let mut outcome = AlmanacOutcome::default();

    let quantum = snapshot
        .quantum
        .map(|q| format!(" · quantum {q} ticks"))
        .unwrap_or_default();
    outcome.push(
        StatusTag::Ok,
        format!(
            "scheduler: {}{quantum} · tick {} · {} schedulable process(es)",
            crate::scheduler::algorithm_label(snapshot.algorithm),
            snapshot.tick,
            snapshot.schedulable_count,
        ),
    );

    for core in &snapshot.cores {
        let occupant = match core.pid {
            Some(pid) => process_label(state, pid),
            None => "idle".to_string(),
        };
        outcome.push(
            StatusTag::Info,
            format!(
                "Core {}: {occupant} · {:.0}% busy",
                core.core,
                core.utilization * 100.0
            ),
        );
    }

    let ready = if snapshot.ready_queue.is_empty() {
        "empty".to_string()
    } else {
        snapshot
            .ready_queue
            .iter()
            .map(|pid| process_label(state, *pid))
            .collect::<Vec<_>>()
            .join(", ")
    };
    outcome.push(StatusTag::Info, format!("ready queue: {ready}"));
    outcome.push(
        StatusTag::Info,
        format!("context switches: {}", snapshot.context_switches),
    );
    outcome.push(
        StatusTag::Info,
        format!(
            "virtual CPU utilization: {:.0}% ({})",
            snapshot.utilization * 100.0,
            snapshot
                .per_core_utilization
                .iter()
                .enumerate()
                .map(|(core, share)| format!("Core {core} {:.0}%", share * 100.0))
                .collect::<Vec<_>>()
                .join(" · "),
        ),
    );
    if snapshot.averages.completed > 0 {
        outcome.push(
            StatusTag::Info,
            format!(
                "averages over {} completed: wait {:.1} · turnaround {:.1} · response {:.1} (ticks)",
                snapshot.averages.completed,
                snapshot.averages.waiting,
                snapshot.averages.turnaround,
                snapshot.averages.response,
            ),
        );
    }
    outcome.push(
        StatusTag::Info,
        "HOST_APP / HOST_COMMAND processes are observed only — Windows schedules them, not Aaru"
            .to_string(),
    );
    Ok(outcome)
}

/// Render `almanac memory` — the simulated RAM / swap / paging status block.
fn memory_status_lines(state: &SystemState) -> Result<AlmanacOutcome, AaruError> {
    let snapshot = state.memory_snapshot()?;
    let mut outcome = AlmanacOutcome::default();

    outcome.push(
        StatusTag::Ok,
        format!(
            "AARU MEMORY — {} replacement · {} MB pages",
            snapshot.policy.label(),
            snapshot.page_size_mb,
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!(
            "RAM: {} / {} MB used · {} MB free",
            snapshot.ram_used_mb, snapshot.ram_total_mb, snapshot.ram_free_mb,
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!(
            "Frames: {} / {} used · {} free",
            snapshot.frames_used, snapshot.frames_total, snapshot.frames_free,
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!(
            "Swap: {} / {} MB used · {} MB free",
            snapshot.swap_used_mb, snapshot.swap_total_mb, snapshot.swap_free_mb,
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!(
            "Page faults: {} · Page hits: {} (swap-out {} · swap-in {})",
            snapshot.page_faults, snapshot.page_hits, snapshot.swap_outs, snapshot.swap_ins,
        ),
    );
    outcome.push(
        StatusTag::Info,
        format!("resident simulated processes: {}", snapshot.processes.len()),
    );
    outcome.push(
        StatusTag::Info,
        "simulated Aaru RAM is independent of Windows host memory".to_string(),
    );

    if let Some(host) = snapshot.host {
        outcome.push(
            StatusTag::Info,
            format!(
                "HOST MEMORY — Windows physical RAM: {} / {} MB used ({}%)",
                host.used_mb, host.total_mb, host.load_percent,
            ),
        );
    }

    Ok(outcome)
}

/// Launch a real editor for `write`/`rewrite … in <App>`.
fn append_editor_launch(
    state: &mut SystemState,
    outcome: &mut AlmanacOutcome,
    cwd: &str,
    app: &str,
    real_path: Option<&str>,
    display: &str,
) -> Result<(), AaruError> {
    match real_path {
        Some(real) => {
            let view = state.open_host_file_in_app(app, real)?;
            outcome.lines.push(super::outcome::OutputLine::new(
                StatusTag::Process,
                format!(
                    "opened {display} in {} — Aaru PID {} [HOST]{}",
                    view.name,
                    view.pid,
                    view.host_pid
                        .map(|pid| format!(", host PID {pid}"))
                        .unwrap_or_default()
                ),
            ));
        }
        None => match state.run_application(cwd, app, &[])? {
            RunReport::HostApp { process } => {
                outcome.lines.push(super::outcome::OutputLine::new(
                    StatusTag::Process,
                    format!(
                        "opened {} — Aaru PID {} [HOST] (the virtual file {display} stays in Aaru)",
                        process.name, process.pid
                    ),
                ));
            }
            RunReport::HostOpen {
                display: app_name, ..
            } => {
                outcome.lines.push(super::outcome::OutputLine::new(
                    StatusTag::Process,
                    format!("opened {app_name} via Windows"),
                ));
            }
            RunReport::HostLaunched { display: app_name } => {
                outcome.lines.push(super::outcome::OutputLine::new(
                    StatusTag::Process,
                    format!("opened {app_name} via Windows (Microsoft Store app)"),
                ));
            }
            RunReport::Builtin { process, window } => {
                outcome.lines.push(super::outcome::OutputLine::new(
                    StatusTag::Process,
                    format!("started {} — Aaru PID {} [SIM]", process.name, process.pid),
                ));
                outcome.open_window_title = window.as_ref().map(|_| process.name.clone());
                outcome.open_window = window;
            }
        },
    }
    Ok(())
}

/// `almanac open <path> [in <app>]`.
///
/// Without `in <app>` this is a plain `cd` into a directory. With `in <app>` it
/// hands the target to that application: a file opens in the editor, a HOST
/// folder opens as a project (`code <dir>`). Virtual resources have no Windows
/// path, so `in <app>` there just launches the app.
fn open_target(
    state: &mut SystemState,
    cwd: &str,
    path: &str,
    editor: EditorTarget,
) -> Result<AlmanacOutcome, AaruError> {
    let app = match editor {
        EditorTarget::None => {
            // Original behaviour: navigate into the directory.
            return Ok(match state.route(cwd, path)? {
                AaruLocation::Virtual(canonical) => {
                    let info = state.open_directory("ROOT", &canonical)?;
                    moved(info.path)
                }
                AaruLocation::HostRoot => moved("HOST"),
                AaruLocation::Host { mount, relative } => {
                    let view = state.host_open(&mount, &relative)?;
                    moved(view.display_path)
                }
            });
        }
        EditorTarget::App(app) => app,
    };

    match state.route(cwd, path)? {
        AaruLocation::HostRoot => Err(AaruError::InvalidPath(
            "HOST is the mount list — open a folder or file inside a mount".to_string(),
        )),
        AaruLocation::Virtual(canonical) => {
            let info = state.inspect("ROOT", &canonical)?;
            let display = format!("AARU>{}", info.path);
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Info,
                if info.metadata.resource_type == ResourceType::Directory {
                    format!("{display} is a virtual directory — launching {app} without a path")
                } else {
                    format!("opening {display}")
                },
            );
            append_editor_launch(state, &mut outcome, cwd, &app, None, &display)?;
            Ok(outcome)
        }
        AaruLocation::Host { mount, relative } => {
            let view = state.host_inspect(&mount, &relative)?;
            let real = view.host_real_path.clone().ok_or_else(|| {
                AaruError::Filesystem("could not resolve the real host path".to_string())
            })?;
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Info,
                format!(
                    "opening {} {} in {app}",
                    if view.is_dir { "folder" } else { "file" },
                    view.display_path
                ),
            );
            append_editor_launch(
                state,
                &mut outcome,
                cwd,
                &app,
                Some(&real),
                &view.display_path,
            )?;
            Ok(outcome)
        }
    }
}

#[allow(clippy::field_reassign_with_default)] // outcome is built up field-by-field
fn run_command_inner(
    state: &mut SystemState,
    cwd: &str,
    command: AlmanacCommand,
) -> Result<AlmanacOutcome, AaruError> {
    Ok(match command {
        AlmanacCommand::Help => help_reference(),

        AlmanacCommand::Open { path, editor } => open_target(state, cwd, &path, editor)?,
        AlmanacCommand::Back => back_target(state, cwd)?,
        AlmanacCommand::Root => {
            state.root()?;
            moved("ROOT")
        }
        AlmanacCommand::Scan => scan_current(state, cwd)?,

        AlmanacCommand::Gen { path } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let info = state.create_directory("ROOT", &canonical)?;
                touched("created directory", &info.path)
            }
            AaruLocation::HostRoot => {
                return Err(AaruError::InvalidPath(
                    "HOST is the mount list — pick a mount, e.g. HOST>Documents>New".to_string(),
                ))
            }
            AaruLocation::Host { mount, relative } => {
                let view = state.host_create_dir(&mount, &relative)?;
                touched("created host directory", &view.display_path)
            }
        },
        AlmanacCommand::Mgen { expression } => {
            if expression.trim_start().starts_with("HOST>") {
                return Err(AaruError::InvalidArgument(
                    "mgen tree generation is virtual-only in this phase; use 'almanac gen' on HOST"
                        .to_string(),
                ));
            }
            let info = state.create_tree(cwd, &expression)?;
            touched("generated tree at", &info.path)
        }

        AlmanacCommand::Write { path, editor } => {
            let (display, real) = match state.route(cwd, &path)? {
                AaruLocation::Virtual(canonical) => {
                    let info = state.create_file("ROOT", &canonical, "")?;
                    (info.path, None)
                }
                AaruLocation::HostRoot => {
                    return Err(AaruError::InvalidPath("HOST is the mount list".to_string()))
                }
                AaruLocation::Host { mount, relative } => {
                    let view = state.host_write(&mount, &relative, "", false)?;
                    (view.display_path, view.host_real_path)
                }
            };
            let mut outcome = touched("created file", &display);
            if let EditorTarget::App(app) = editor {
                append_editor_launch(state, &mut outcome, cwd, &app, real.as_deref(), &display)?;
            }
            outcome
        }
        AlmanacCommand::Rewrite { path, editor } => {
            let (display, real) = match state.route(cwd, &path)? {
                AaruLocation::Virtual(canonical) => {
                    let info = state.inspect("ROOT", &canonical)?;
                    if info.metadata.resource_type != ResourceType::File {
                        return Err(AaruError::NotAFile(info.path));
                    }
                    // Touch modified time / confirm writability.
                    let text = state.read_file("ROOT", &canonical)?;
                    state.write_file("ROOT", &canonical, &text)?;
                    (info.path, None)
                }
                AaruLocation::HostRoot => {
                    return Err(AaruError::InvalidPath("HOST is the mount list".to_string()))
                }
                AaruLocation::Host { mount, relative } => {
                    let text = state.host_read(&mount, &relative)?;
                    let view = state.host_write(&mount, &relative, &text, true)?;
                    (view.display_path, view.host_real_path)
                }
            };
            let mut outcome = AlmanacOutcome::line(StatusTag::Info, format!("editing {display}"));
            match editor {
                EditorTarget::App(app) => {
                    append_editor_launch(state, &mut outcome, cwd, &app, real.as_deref(), &display)?
                }
                EditorTarget::None => outcome.push(
                    StatusTag::Info,
                    "add 'in <App>' to open it in an editor, or 'almanac reveal <path>' for the \
                     default app"
                        .to_string(),
                ),
            }
            outcome
        }

        AlmanacCommand::Destroy { path } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let summary = state.delete_preview("ROOT", &canonical)?;
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::DestroyConfirm {
                    path: canonical.clone(),
                    total: summary.total_resources,
                });
                let mut outcome = AlmanacOutcome::default();
                outcome.push(
                    StatusTag::Info,
                    format!(
                        "{canonical}: {} file(s), {} director(y/ies), {} total resource(s) affected",
                        summary.files, summary.directories, summary.total_resources
                    ),
                );
                outcome.prompt = Some(PromptRequest {
                    id,
                    kind: "destroy_confirm".to_string(),
                    message: "Delete these resources? (Y/N)".to_string(),
                    masked: false,
                });
                outcome
            }
            AaruLocation::HostRoot => {
                return Err(AaruError::PermissionDenied(
                    "cannot delete HOST — use 'almanac unmount <alias>'".to_string(),
                ))
            }
            AaruLocation::Host { mount, relative } => {
                let (files, folders) = state.host_delete_preview(&mount, &relative)?;
                let display = format!("HOST>{mount}>{}", relative.join(">"));
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::DestroyHostConfirm {
                    alias: mount.clone(),
                    relative: relative.clone(),
                    display: display.clone(),
                });
                let mut outcome = AlmanacOutcome::default();
                outcome.push(
                    StatusTag::Locked,
                    "AARU::DESTROY [HOST RESOURCE]".to_string(),
                );
                outcome.push(StatusTag::Info, format!("target: {display}"));
                outcome.push(StatusTag::Info, format!("files: {files}"));
                outcome.push(StatusTag::Info, format!("folders: {folders}"));
                outcome.push(
                    StatusTag::Info,
                    "this affects physical files on this computer".to_string(),
                );
                outcome.prompt = Some(PromptRequest {
                    id,
                    kind: "destroy_host_confirm".to_string(),
                    message: "Move these resources to the Recycle Bin? (Y/N)".to_string(),
                    masked: false,
                });
                outcome
            }
        },

        AlmanacCommand::Rename { path, new_name } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let info = state.rename("ROOT", &canonical, &new_name)?;
                touched("renamed to", &info.path)
            }
            AaruLocation::HostRoot => {
                return Err(AaruError::PermissionDenied(
                    "cannot rename HOST".to_string(),
                ))
            }
            AaruLocation::Host { mount, relative } => {
                let view = state.host_rename(&mount, &relative, &new_name)?;
                touched("renamed to", &view.display_path)
            }
        },
        AlmanacCommand::Transfer { from, to } => relocate(state, cwd, &from, &to, false)?,
        AlmanacCommand::Copy { from, to } => relocate(state, cwd, &from, &to, true)?,

        AlmanacCommand::Lookout { query } => {
            let results = state.lookout(&query)?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(
                StatusTag::Ok,
                format!("{} match(es) for \"{query}\"", results.hits.len()),
            );
            for (index, hit) in results.hits.iter().enumerate() {
                outcome.push(StatusTag::Info, format!("[{}] {}", index + 1, hit.display));
            }
            for skipped in &results.skipped {
                outcome.push(
                    StatusTag::Locked,
                    format!("skipped locked subtree AARU>{skipped}"),
                );
            }
            outcome
        }
        AlmanacCommand::Inspect { path } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let info = state.inspect("ROOT", &canonical)?;
                let meta = &info.metadata;
                let view = EntryView {
                    display_path: format!("AARU>{}", info.path),
                    name: meta.name.clone(),
                    kind: ProviderKind::Virtual,
                    is_dir: meta.resource_type == ResourceType::Directory,
                    size: meta.size,
                    modified_ms: Some(meta.modified_at_ms),
                    created_ms: Some(meta.created_at_ms),
                    read_only: !meta.permissions.write,
                    aaru_locked: meta.locked,
                    host_real_path: None,
                };
                let mut outcome = inspect_lines(&view);
                outcome.push(
                    StatusTag::Info,
                    format!(
                        "permissions: {}{}{}",
                        if meta.permissions.read { "R" } else { "-" },
                        if meta.permissions.write { "W" } else { "-" },
                        if meta.permissions.execute { "X" } else { "-" },
                    ),
                );
                outcome
            }
            AaruLocation::HostRoot => {
                let mut outcome = AlmanacOutcome::line(StatusTag::Ok, "inspect HOST");
                for mount in state.host_mount_list()? {
                    outcome.push(
                        StatusTag::Info,
                        format!(
                            "{}{}  → {}{}",
                            mount.alias,
                            if mount.is_default { " (default)" } else { "" },
                            mount.source,
                            if mount.available { "" } else { "  [missing]" }
                        ),
                    );
                }
                outcome
            }
            AaruLocation::Host { mount, relative } => {
                let view = state.host_inspect(&mount, &relative)?;
                inspect_lines(&view)
            }
        },

        AlmanacCommand::Lock { path } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let display = state.precheck_lock("ROOT", &canonical)?;
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::LockPassword {
                    path: display.clone(),
                });
                lock_prompt(
                    id,
                    "lock_password",
                    "New lock password:",
                    &format!("locking {display}"),
                )
            }
            AaruLocation::HostRoot => {
                return Err(AaruError::PermissionDenied("cannot lock HOST".to_string()))
            }
            AaruLocation::Host { mount, relative } => {
                let (canonical_id, display) = state.host_precheck_lock(&mount, &relative)?;
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::HostLockPassword {
                    canonical_id,
                    display: display.clone(),
                });
                let mut outcome = lock_prompt(
                    id,
                    "host_lock_password",
                    "New Aaru lock password:",
                    &format!("locking {display}"),
                );
                outcome.push(
                    StatusTag::Info,
                    "note: an Aaru lock only gates access inside Aaru-OS. It does not \
                     encrypt the folder, change Windows permissions, or hide it from other \
                     programs."
                        .to_string(),
                );
                outcome
            }
        },
        AlmanacCommand::Unlock { path } => match state.route(cwd, &path)? {
            AaruLocation::Virtual(canonical) => {
                let display = state.precheck_unlock("ROOT", &canonical)?;
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::UnlockPassword {
                    path: display.clone(),
                });
                lock_prompt(
                    id,
                    "unlock_password",
                    "Lock password:",
                    &format!("unlocking {display}"),
                )
            }
            AaruLocation::HostRoot => {
                return Err(AaruError::PermissionDenied(
                    "cannot unlock HOST".to_string(),
                ))
            }
            AaruLocation::Host { mount, relative } => {
                let (canonical_id, display) = state.host_precheck_unlock(&mount, &relative)?;
                let id = state.next_prompt_id();
                state.set_pending_prompt(PendingPrompt::HostUnlockPassword {
                    canonical_id,
                    display: display.clone(),
                });
                lock_prompt(
                    id,
                    "host_unlock_password",
                    "Aaru lock password:",
                    &format!("unlocking {display}"),
                )
            }
        },

        AlmanacCommand::Mount { path } => match path {
            None => {
                let mut outcome =
                    AlmanacOutcome::line(StatusTag::Info, "choose a folder to expose under HOST…");
                outcome.request_mount = true;
                outcome
            }
            Some(path) => {
                let alias = state.host_mount(Path::new(&path), None)?;
                AlmanacOutcome::line(StatusTag::Ok, format!("mounted HOST>{alias}  →  {path}"))
            }
        },
        AlmanacCommand::Unmount { alias } => {
            state.host_unmount(&alias)?;
            AlmanacOutcome::line(StatusTag::Ok, format!("unmounted HOST>{alias}"))
        }
        AlmanacCommand::Mounts => {
            let mounts = state.host_mount_list()?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(StatusTag::Ok, format!("{} host mount(s)", mounts.len()));
            for mount in mounts {
                outcome.push(
                    StatusTag::Info,
                    format!(
                        "HOST>{}{}  →  {}{}",
                        mount.alias,
                        if mount.is_default { "  (default)" } else { "" },
                        mount.source,
                        if mount.available { "" } else { "  [missing]" }
                    ),
                );
            }
            outcome
        }

        AlmanacCommand::Run { application, args } => {
            launch_application(state, cwd, &application, &args)?
        }

        AlmanacCommand::Reveal { path } => {
            match state.route(cwd, &path)? {
                AaruLocation::Host { mount, relative } => {
                    let view = state.host_inspect(&mount, &relative)?;
                    let real = view.host_real_path.clone().ok_or_else(|| {
                        AaruError::Filesystem("could not resolve the real host path".to_string())
                    })?;
                    let mut outcome = AlmanacOutcome::line(
                        StatusTag::Process,
                        format!(
                            "opening {} with its default Windows application",
                            view.display_path
                        ),
                    );
                    outcome.push(
                        StatusTag::Info,
                        "handed to Windows — not tracked as an Aaru process".to_string(),
                    );
                    outcome.launch = Some(AppLaunch {
                        app: "$default".to_string(),
                        path: Some(real),
                        args: Vec::new(),
                    });
                    outcome
                }
                _ => return Err(AaruError::InvalidPath(
                    "reveal applies only to HOST resources (virtual files have no Windows path)"
                        .to_string(),
                )),
            }
        }

        AlmanacCommand::Process => {
            let processes = state.process_list()?;
            let mut outcome = AlmanacOutcome::default();
            outcome.push(
                StatusTag::Ok,
                format!(
                    "{} process(es) — [SIM] simulated · [HOST] Windows-managed",
                    processes.len()
                ),
            );
            for process in &processes {
                outcome.push(StatusTag::Info, format_process_row(process));
            }
            outcome
        }
        AlmanacCommand::Terminate { pid } => {
            let view = state.process_terminate(pid)?;
            AlmanacOutcome::line(
                StatusTag::Ok,
                format!("terminated PID {} ({})", view.pid, view.name),
            )
        }
        AlmanacCommand::Suspend { pid } => {
            let view = state.process_suspend(pid)?;
            AlmanacOutcome::line(
                StatusTag::Ok,
                format!(
                    "suspended PID {} ({}) — simulated scheduler state",
                    view.pid, view.name
                ),
            )
        }
        AlmanacCommand::Resume { pid } => {
            let view = state.process_resume(pid)?;
            AlmanacOutcome::line(
                StatusTag::Ok,
                format!(
                    "resumed PID {} ({}) — back in the virtual CPU's READY queue",
                    view.pid, view.name
                ),
            )
        }

        AlmanacCommand::Scheduler => scheduler_status_lines(state)?,
        AlmanacCommand::SchedulerChange { algorithm } => {
            let label = crate::scheduler::algorithm_label(algorithm);
            state.scheduler_set_algorithm(algorithm)?;
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Ok,
                format!("virtual CPU scheduler switched to {label}"),
            );
            for line in scheduler_status_lines(state)?.lines {
                outcome.lines.push(line);
            }
            outcome
        }
        AlmanacCommand::SchedulerTick { ticks } => {
            state.require_authentication()?;
            state.scheduler_tick(ticks);
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Ok,
                format!("advanced the virtual CPU by {ticks} simulation tick(s)"),
            );
            for line in scheduler_status_lines(state)?.lines {
                outcome.lines.push(line);
            }
            outcome
        }

        AlmanacCommand::Memory => memory_status_lines(state)?,
        AlmanacCommand::MemorySetPolicy { policy } => {
            state.memory_set_policy(policy)?;
            let mut outcome = AlmanacOutcome::line(
                StatusTag::Ok,
                format!("page replacement policy set to {}", policy.label()),
            );
            for line in memory_status_lines(state)?.lines {
                outcome.lines.push(line);
            }
            outcome
        }

        AlmanacCommand::Logout => {
            state.require_authentication()?;
            let id = state.next_prompt_id();
            state.set_pending_prompt(PendingPrompt::LogoutPassword);
            let mut outcome = AlmanacOutcome::default();
            outcome.prompt = Some(PromptRequest {
                id,
                kind: "logout_password".to_string(),
                message: "Password:".to_string(),
                masked: true,
            });
            outcome
        }
        AlmanacCommand::KillLapsession => {
            state.require_authentication()?;
            let summary = state.lifecycle_summary();
            let id = state.next_prompt_id();
            state.set_pending_prompt(PendingPrompt::KillLapsessionConfirm);
            let mut outcome = AlmanacOutcome::line(
                StatusTag::System,
                format!(
                    "Kill LapSession will close Aaru only — {} Aaru process(es), {} tracked host process(es) active",
                    summary.running_aaru, summary.running_host
                ),
            );
            if !summary.host_names.is_empty() {
                outcome.push(
                    StatusTag::Process,
                    format!("tracked host processes: {}", summary.host_names.join(", ")),
                );
            }
            outcome.push(
                StatusTag::Info,
                "Windows and unrelated Windows processes will not be shut down or targeted",
            );
            outcome.prompt = Some(PromptRequest {
                id,
                kind: "kill_lapsession_confirm".to_string(),
                message: "Close Aaru-OS and its managed processes? (Y/N)".to_string(),
                masked: false,
            });
            outcome
        }
        AlmanacCommand::Hibernate => {
            state.require_authentication()?;
            state.prepare_hibernate(
                "ROOT".to_string(),
                serde_json::Value::Null,
                serde_json::Value::Null,
            )?;
            let mut outcome = AlmanacOutcome::line(
                StatusTag::System,
                "Aaru runtime hibernated — simulated processes, scheduler and memory saved",
            );
            outcome.system_action = Some(SystemAction::Hibernate);
            outcome
        }
        AlmanacCommand::Restart => {
            state.require_authentication()?;
            state.prepare_restart()?;
            let mut outcome = AlmanacOutcome::line(
                StatusTag::System,
                "restarting Aaru-OS — running processes will not be restored",
            );
            outcome.system_action = Some(SystemAction::Restart);
            outcome
        }
    })
}

#[allow(clippy::field_reassign_with_default)] // outcome is built up field-by-field
fn respond_inner(state: &mut SystemState, response: &str) -> Result<AlmanacOutcome, AaruError> {
    match state.take_pending_prompt() {
        PendingPrompt::None => Err(AaruError::NoPendingPrompt),

        PendingPrompt::DestroyConfirm { path, total } => {
            if is_affirmative(response) {
                let summary = state.delete_recursive("ROOT", &path)?;
                Ok(AlmanacOutcome::line(
                    StatusTag::Ok,
                    format!(
                        "destroyed {path} — {} resource(s) removed",
                        summary.total_resources
                    ),
                ))
            } else {
                Ok(AlmanacOutcome::line(
                    StatusTag::Info,
                    format!("destroy cancelled — {path} untouched ({total} resource(s))"),
                ))
            }
        }

        PendingPrompt::LockPassword { path } => {
            let id = state.next_prompt_id();
            state.set_pending_prompt(PendingPrompt::LockConfirm {
                path,
                first: Zeroizing::new(response.to_string()),
            });
            let mut outcome = AlmanacOutcome::default();
            outcome.prompt = Some(PromptRequest {
                id,
                kind: "lock_confirm".to_string(),
                message: "Confirm lock password:".to_string(),
                masked: true,
            });
            Ok(outcome)
        }
        PendingPrompt::LockConfirm { path, first } => {
            if response != first.as_str() {
                return Ok(AlmanacOutcome::line(
                    StatusTag::Error,
                    "passwords did not match — lock not created",
                ));
            }
            state.lock_resource("ROOT", &path, response)?;
            Ok(AlmanacOutcome::line(
                StatusTag::Ok,
                format!("locked {path} — authentication is now required to enter it"),
            ))
        }

        PendingPrompt::UnlockPassword { path } => {
            match state.unlock_resource("ROOT", &path, response) {
                Ok(_) => Ok(AlmanacOutcome::line(
                    StatusTag::Ok,
                    format!("unlocked {path}"),
                )),
                Err(error @ AaruError::AuthenticationFailed) => {
                    Ok(AlmanacOutcome::line(StatusTag::Denied, error.to_string()))
                }
                Err(error @ AaruError::AccountLocked { .. }) => {
                    Ok(AlmanacOutcome::line(StatusTag::Auth, error.to_string()))
                }
                Err(error) => Err(error),
            }
        }

        PendingPrompt::LogoutPassword => {
            if state.verify_login_password(response) {
                state.logout();
                let mut outcome = AlmanacOutcome::line(StatusTag::System, "logged out");
                outcome.system_action = Some(SystemAction::LoggedOut);
                Ok(outcome)
            } else {
                Ok(AlmanacOutcome::line(
                    StatusTag::Denied,
                    "password incorrect — you are still logged in",
                ))
            }
        }

        PendingPrompt::KillLapsessionConfirm => {
            if is_affirmative(response) {
                state.prepare_shutdown()?;
                let mut outcome = AlmanacOutcome::line(
                    StatusTag::System,
                    "Aaru persistent state saved; managed processes released; closing LapSession",
                );
                outcome.system_action = Some(SystemAction::Shutdown);
                Ok(outcome)
            } else {
                Ok(AlmanacOutcome::line(
                    StatusTag::Info,
                    "Kill LapSession cancelled — Aaru remains running",
                ))
            }
        }

        // ---- host prompt handlers ----
        PendingPrompt::DestroyHostConfirm {
            alias,
            relative,
            display,
        } => {
            if is_affirmative(response) {
                let outcome = state.host_recycle(&alias, &relative)?;
                Ok(AlmanacOutcome::line(
                    StatusTag::Ok,
                    format!(
                        "{display} moved to the Recycle Bin — {} file(s), {} folder(s)",
                        outcome.files, outcome.folders
                    ),
                ))
            } else {
                Ok(AlmanacOutcome::line(
                    StatusTag::Info,
                    format!("cancelled — {display} was not touched"),
                ))
            }
        }
        PendingPrompt::HostLockPassword {
            canonical_id,
            display,
        } => {
            let id = state.next_prompt_id();
            state.set_pending_prompt(PendingPrompt::HostLockConfirm {
                canonical_id,
                display,
                first: Zeroizing::new(response.to_string()),
            });
            let mut outcome = AlmanacOutcome::default();
            outcome.prompt = Some(PromptRequest {
                id,
                kind: "host_lock_confirm".to_string(),
                message: "Confirm Aaru lock password:".to_string(),
                masked: true,
            });
            Ok(outcome)
        }
        PendingPrompt::HostLockConfirm {
            canonical_id,
            display,
            first,
        } => {
            if response != first.as_str() {
                return Ok(AlmanacOutcome::line(
                    StatusTag::Error,
                    "passwords did not match — lock not created",
                ));
            }
            state.host_commit_lock(&canonical_id, response)?;
            Ok(AlmanacOutcome::line(
                StatusTag::Ok,
                format!(
                    "{display} now carries an Aaru access lock (Aaru-OS only — Windows is unaffected)"
                ),
            ))
        }
        PendingPrompt::HostUnlockPassword {
            canonical_id,
            display,
        } => match state.host_commit_unlock(&canonical_id, response) {
            Ok(()) => Ok(AlmanacOutcome::line(
                StatusTag::Ok,
                format!("removed the Aaru lock on {display}"),
            )),
            Err(error @ AaruError::AuthenticationFailed) => {
                Ok(AlmanacOutcome::line(StatusTag::Denied, error.to_string()))
            }
            Err(error @ AaruError::AccountLocked { .. }) => {
                Ok(AlmanacOutcome::line(StatusTag::Auth, error.to_string()))
            }
            Err(error) => Err(error),
        },
    }
}

fn is_affirmative(response: &str) -> bool {
    let response = response.trim();
    response.eq_ignore_ascii_case("y") || response.eq_ignore_ascii_case("yes")
}

fn help_reference() -> AlmanacOutcome {
    let mut outcome = AlmanacOutcome::default();
    outcome.push(StatusTag::Info, "Almanac native commands:");
    for (verb, blurb) in [
        (
            "open <path> [in <App>]",
            "enter a directory, or open a file/HOST folder in an app",
        ),
        ("back", "go to the parent directory"),
        ("root", "go to ROOT"),
        ("scan", "list the current directory"),
        ("gen <path>", "create a directory"),
        (
            "mgen <expr>",
            "create a directory tree, e.g. Projects>(A,B,C)",
        ),
        (
            "write <file> [in <App>]",
            "create a file (optionally open it)",
        ),
        ("rewrite <file> [in <App>]", "edit an existing file"),
        ("destroy <path>", "delete a subtree (asks for confirmation)"),
        ("rename <path>>newName", "rename a resource"),
        (
            "transfer <from> <to>",
            "move a resource (HOST↔AARU allowed)",
        ),
        ("copy <from> <to>", "copy a resource (HOST↔AARU allowed)"),
        ("lookout <term>", "search accessible resources"),
        ("inspect <path>", "show resource metadata"),
        ("lock <path>", "password-lock a directory"),
        ("unlock <path>", "remove a directory lock"),
        (
            "mount [path] / unmount <alias> / mounts",
            "manage host folders",
        ),
        (
            "run <App> [args]",
            "launch a built-in app or a real host app",
        ),
        (
            "reveal HOST><path>",
            "open a host file in its default Windows app",
        ),
        ("process", "list the Aaru process table"),
        ("terminate|suspend|resume <pid>", "manage an Aaru process"),
        (
            "scheduler [change <algo>] [tick <n>]",
            "inspect or drive the virtual CPU scheduler",
        ),
        (
            "memory [policy <FIFO|LRU>]",
            "inspect the simulated RAM / swap / paging model",
        ),
        ("logout", "end the session"),
        ("kill lapsession", "shut Aaru-OS down"),
        ("hibernate", "hibernate Aaru-OS"),
        ("restart", "restart Aaru-OS"),
    ] {
        outcome.push(StatusTag::Info, format!("  almanac {verb} — {blurb}"));
    }
    outcome.push(
        StatusTag::Info,
        "quick launch: claude, chatgpt, google, vsc, antigravity, brave, chrome",
    );
    outcome.push(
        StatusTag::Info,
        "anything else (npm, git, python, …) runs on the host shell",
    );
    outcome
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::JsonPersistence;

    fn logged_in_state(directory: &tempfile::TempDir) -> SystemState {
        let mut state =
            SystemState::fresh(JsonPersistence::new(directory.path().join("state.json")));
        state.configure_login("login-password").unwrap();
        state
    }

    #[test]
    fn arrow_paths_route_to_almanac_and_shell_redirection_routes_to_host() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        state
            .create_directory("ROOT>Documents", "Projects")
            .unwrap();

        let almanac = evaluate(&mut state, "ROOT", "almanac open Documents>Projects");
        assert_eq!(almanac.new_cwd.as_deref(), Some("ROOT>Documents>Projects"));
        assert!(almanac.shell.is_none());

        let host = evaluate(&mut state, "ROOT", "echo hello > output.txt");
        let spec = host
            .shell
            .expect("redirection line must go to the host shell");
        assert_eq!(spec.program, "echo");
        assert_eq!(spec.args, vec!["hello", ">", "output.txt"]);
    }

    #[test]
    fn case_sensitive_execution_even_though_completion_is_not() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        assert!(matches!(
            evaluate(&mut state, "ROOT", "almanac open documents").first_tag(),
            Some(StatusTag::Error)
        ));
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac open Documents")
                .new_cwd
                .as_deref(),
            Some("ROOT>Documents")
        );
    }

    #[test]
    fn mgen_builds_the_tree_and_rejects_bad_expressions() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let ok = evaluate(
            &mut state,
            "ROOT>Projects",
            "almanac mgen Workspace>(Frontend,Backend,Docs)",
        );
        assert_eq!(ok.first_tag(), Some(StatusTag::Ok));
        assert!(state.inspect("ROOT", "Projects>Workspace>Frontend").is_ok());
        assert!(state.inspect("ROOT", "Projects>Workspace>Docs").is_ok());

        let nested = evaluate(
            &mut state,
            "ROOT>Projects",
            "almanac mgen file1>(file2>(file4),file3)",
        );
        assert_eq!(nested.first_tag(), Some(StatusTag::Ok));
        assert!(state.inspect("ROOT", "Projects>file1>file2>file4").is_ok());

        let bad = evaluate(&mut state, "ROOT>Projects", "almanac mgen Broken>(A,");
        assert_eq!(bad.first_tag(), Some(StatusTag::Error));
    }

    #[test]
    fn destroy_counts_then_requires_confirmation() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        state
            .create_tree("ROOT>Projects", "Doomed>(One,Two)")
            .unwrap();
        state
            .create_file("ROOT>Projects>Doomed>One", "note.txt", "hi")
            .unwrap();

        let prompt = evaluate(&mut state, "ROOT", "almanac destroy Projects>Doomed");
        let request = prompt.prompt.expect("destroy must prompt");
        assert_eq!(request.kind, "destroy_confirm");
        assert!(!request.masked);
        assert!(prompt.lines[0].text.contains("1 file(s)"));
        assert!(prompt.lines[0].text.contains("3 director"));

        let cancelled = respond(&mut state, "n");
        assert_eq!(cancelled.first_tag(), Some(StatusTag::Info));
        assert!(state.inspect("ROOT", "Projects>Doomed").is_ok());

        evaluate(&mut state, "ROOT", "almanac destroy Projects>Doomed");
        let done = respond(&mut state, "Y");
        assert_eq!(done.first_tag(), Some(StatusTag::Ok));
        assert!(state.inspect("ROOT", "Projects>Doomed").is_err());
    }

    #[test]
    fn interactive_lock_then_unlock_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let start = evaluate(&mut state, "ROOT", "almanac lock Projects");
        assert_eq!(start.prompt.as_ref().unwrap().kind, "lock_password");
        assert!(start.prompt.as_ref().unwrap().masked);

        let confirm = respond(&mut state, "vault-password");
        assert_eq!(confirm.prompt.as_ref().unwrap().kind, "lock_confirm");

        let locked = respond(&mut state, "vault-password");
        assert_eq!(locked.first_tag(), Some(StatusTag::Ok));

        // Entering the locked directory now fails until authenticated.
        let blocked = evaluate(&mut state, "ROOT", "almanac open Projects");
        assert_eq!(blocked.first_tag(), Some(StatusTag::Locked));

        evaluate(&mut state, "ROOT", "almanac unlock Projects");
        let unlocked = respond(&mut state, "vault-password");
        assert_eq!(unlocked.first_tag(), Some(StatusTag::Ok));
        assert!(evaluate(&mut state, "ROOT", "almanac open Projects")
            .new_cwd
            .is_some());
    }

    #[test]
    fn lock_password_mismatch_aborts_without_locking() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        evaluate(&mut state, "ROOT", "almanac lock Projects");
        respond(&mut state, "first-secret");
        let result = respond(&mut state, "second-secret");
        assert_eq!(result.first_tag(), Some(StatusTag::Error));
        assert!(!state.has_pending_prompt());
        // Projects was never locked.
        assert!(evaluate(&mut state, "ROOT", "almanac open Projects")
            .new_cwd
            .is_some());
    }

    #[test]
    fn passwords_never_enter_history_but_commands_do() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        state.record_command_history("almanac scan");
        evaluate(&mut state, "ROOT", "almanac lock Projects");
        // respond() is what the command layer calls for prompt input — it must
        // never touch history.
        respond(&mut state, "super-secret-pw");
        respond(&mut state, "super-secret-pw");

        let history = state.command_history();
        assert!(history.contains(&"almanac scan".to_string()));
        assert!(!history
            .iter()
            .any(|entry| entry.contains("super-secret-pw")));
    }

    #[test]
    fn unknown_shell_command_is_not_a_parser_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        let outcome = evaluate(&mut state, "ROOT", "git status");
        assert!(outcome.shell.is_some());
        assert!(outcome
            .lines
            .iter()
            .all(|line| line.tag != StatusTag::Error));
        let spec = outcome.shell.unwrap();
        assert_eq!(spec.program, "git");
        assert_eq!(spec.args, vec!["status"]);
    }

    #[test]
    fn direct_application_names_bypass_the_host_shell() {
        assert_eq!(direct_app_shortcut("chatgpt").unwrap().0, "ChatGPT");
        assert_eq!(direct_app_shortcut("CLAUDE").unwrap().0, "Claude");
        assert_eq!(direct_app_shortcut("brave").unwrap().0, "Brave");
        assert_eq!(direct_app_shortcut("Chrome").unwrap().0, "Chrome");
        assert_eq!(direct_app_shortcut("google").unwrap().0, "Chrome");
        assert_eq!(direct_app_shortcut("VSC").unwrap().0, "VSCode");
        assert_eq!(direct_app_shortcut("antigravity").unwrap().0, "Antigravity");
        assert!(direct_app_shortcut("git status").is_none());
    }

    // -------------------------- Phase 4: host bridge --------------------------

    /// A state with one temp host mount aliased `Dev`, pre-seeded with files.
    fn host_state(dir: &tempfile::TempDir, work: &std::path::Path) -> SystemState {
        let mut state = logged_in_state(dir);
        std::fs::create_dir(work.join("University")).unwrap();
        std::fs::write(work.join("University").join("report.pdf"), b"pdf").unwrap();
        std::fs::write(work.join("notes.txt"), b"draft").unwrap();
        state.host_mount(work, Some("Dev")).unwrap();
        state
    }

    #[test]
    fn engine_routes_host_and_virtual_paths_separately() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        let host = evaluate(&mut state, "ROOT", "almanac open HOST>Dev>University");
        assert_eq!(host.new_cwd.as_deref(), Some("HOST>Dev>University"));

        let virt = evaluate(&mut state, "ROOT", "almanac open Documents");
        assert_eq!(virt.new_cwd.as_deref(), Some("ROOT>Documents"));
    }

    #[test]
    fn host_inspect_reports_real_metadata_and_path() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        let outcome = evaluate(&mut state, "ROOT", "almanac inspect HOST>Dev>notes.txt");
        assert_eq!(outcome.first_tag(), Some(StatusTag::Ok));
        let text = outcome.rendered().join("\n");
        assert!(text.contains("Windows host filesystem"));
        assert!(text.contains("host path:"));
    }

    #[test]
    fn host_write_rewrite_copy_move_and_search_flow() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac write HOST>Dev>fresh.txt").first_tag(),
            Some(StatusTag::Ok)
        );
        assert!(work.path().join("fresh.txt").is_file());
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac rewrite HOST>Dev>fresh.txt").first_tag(),
            Some(StatusTag::Info)
        );
        // rewrite requires an existing file
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac rewrite HOST>Dev>missing.txt").first_tag(),
            Some(StatusTag::Error)
        );

        evaluate(&mut state, "ROOT", "almanac gen HOST>Dev>Archive");
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac copy HOST>Dev>notes.txt HOST>Dev>Archive"
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert!(work.path().join("Archive").join("notes.txt").is_file());
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac transfer HOST>Dev>fresh.txt HOST>Dev>Archive"
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert!(!work.path().join("fresh.txt").exists());

        let search = evaluate(&mut state, "ROOT", "almanac lookout report");
        let joined = search.rendered().join("\n");
        assert!(joined.contains("HOST>Dev>University>report.pdf"));
    }

    #[test]
    fn cross_boundary_copy_host_to_aaru_then_transfer_aaru_to_host() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        // HOST → AARU copy: the host file lands in the virtual tree, original stays.
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac copy HOST>Dev>notes.txt Documents"
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            state.read_file("ROOT", "ROOT>Documents>notes.txt").unwrap(),
            "draft"
        );
        assert!(work.path().join("notes.txt").is_file());

        // HOST → AARU copy of a whole folder.
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac copy HOST>Dev>University Documents"
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            state
                .read_file("ROOT", "ROOT>Documents>University>report.pdf")
                .unwrap(),
            "pdf"
        );

        // AARU → HOST transfer: the virtual file is written to the mount and
        // then removed from the Aaru filesystem.
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac transfer AARU>Documents>notes.txt HOST>Dev>University"
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            std::fs::read_to_string(work.path().join("University").join("notes.txt")).unwrap(),
            "draft"
        );
        assert!(state.read_file("ROOT", "ROOT>Documents>notes.txt").is_err());
    }

    #[test]
    fn cross_boundary_copy_round_trips_binary_files_and_spaced_names() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        // A non-UTF-8 file and a name with spaces — both previously rejected.
        // A path segment containing spaces has to be quoted on the command line.
        let blob: Vec<u8> = vec![0x00, 0xff, 0x10, 0x80, b'P', b'K', 0x03, 0x04];
        std::fs::write(work.path().join("logo bits.bin"), &blob).unwrap();

        let outcome = evaluate(
            &mut state,
            "ROOT",
            "almanac copy \"HOST>Dev>logo bits.bin\" Downloads",
        );
        assert_eq!(outcome.first_tag(), Some(StatusTag::Ok));
        assert_eq!(
            state
                .read_file_bytes("ROOT", "ROOT>Downloads>logo bits.bin")
                .unwrap(),
            blob
        );
        // Reading it as text is refused — it is binary.
        assert!(state
            .read_file("ROOT", "ROOT>Downloads>logo bits.bin")
            .is_err());

        // AARU → HOST: bytes come back out identical.
        assert_eq!(
            evaluate(
                &mut state,
                "ROOT",
                "almanac copy \"AARU>Downloads>logo bits.bin\" HOST>Dev>University",
            )
            .first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            std::fs::read(work.path().join("University").join("logo bits.bin")).unwrap(),
            blob
        );
    }

    #[test]
    fn cross_boundary_copy_skips_oversize_files_and_keeps_going() {
        use crate::state::MAX_CROSS_COPY_FILE_BYTES;
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        std::fs::create_dir(work.path().join("mixed")).unwrap();
        std::fs::write(work.path().join("mixed").join("small.txt"), b"keep me").unwrap();
        std::fs::write(
            work.path().join("mixed").join("huge.bin"),
            vec![7u8; (MAX_CROSS_COPY_FILE_BYTES + 1) as usize],
        )
        .unwrap();

        let outcome = evaluate(
            &mut state,
            "ROOT",
            "almanac transfer HOST>Dev>mixed Documents",
        );
        let rendered = outcome.rendered().join("\n");

        // The small file made it; the oversize one was skipped and reported.
        assert_eq!(
            state
                .read_file("ROOT", "ROOT>Documents>mixed>small.txt")
                .unwrap(),
            "keep me"
        );
        assert!(state
            .read_file_bytes("ROOT", "ROOT>Documents>mixed>huge.bin")
            .is_err());
        assert!(rendered.contains("1 entr"));
        assert!(rendered.contains("cross-copy limit"));
        // transfer must NOT delete the source when anything was skipped.
        assert!(work.path().join("mixed").join("huge.bin").is_file());
    }

    #[test]
    fn cross_boundary_copy_rejects_bare_root_endpoints() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());
        // The bare mount root is not a valid source — pick something inside it.
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac copy HOST>Dev Documents").first_tag(),
            Some(StatusTag::Error)
        );
        // The bare HOST mount list is not a valid destination.
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac transfer Documents HOST").first_tag(),
            Some(StatusTag::Error)
        );
    }

    #[test]
    fn engine_rejects_host_traversal() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());
        let outcome = evaluate(&mut state, "ROOT", "almanac inspect HOST>Dev>..>..>secrets");
        assert!(matches!(
            outcome.first_tag(),
            Some(StatusTag::Denied) | Some(StatusTag::Error)
        ));
    }

    #[test]
    fn host_destroy_counts_and_labels_then_recycles_or_documents() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());

        let prompt = evaluate(&mut state, "ROOT", "almanac destroy HOST>Dev>University");
        assert_eq!(
            prompt.prompt.as_ref().map(|request| request.kind.as_str()),
            Some("destroy_host_confirm")
        );
        let rendered = prompt.rendered().join("\n");
        assert!(rendered.contains("[HOST RESOURCE]"));
        assert!(rendered.contains("files: 1"));
        assert!(rendered.contains("folders: 1"));
        assert!(rendered.contains("physical files on this computer"));

        let done = respond(&mut state, "y");
        // The Recycle Bin move either succeeds, or fails loudly — never a
        // silent permanent delete.
        match done.first_tag() {
            Some(StatusTag::Ok) => assert!(!work.path().join("University").exists()),
            Some(StatusTag::Error) => {
                assert!(done.rendered().join("\n").contains("Recycle Bin"));
                assert!(work.path().join("University").exists());
            }
            other => panic!("unexpected host destroy outcome: {other:?}"),
        }
    }

    #[test]
    fn locked_host_directory_is_gated_until_unlocked() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());
        std::fs::create_dir(work.path().join("Secret")).unwrap();
        std::fs::write(work.path().join("Secret").join("plan.txt"), b"x").unwrap();

        evaluate(&mut state, "ROOT", "almanac lock HOST>Dev>Secret");
        respond(&mut state, "vault-password");
        assert_eq!(
            respond(&mut state, "vault-password").first_tag(),
            Some(StatusTag::Ok)
        );

        state.logout();
        state.login("login-password").unwrap();

        let blocked = evaluate(&mut state, "ROOT", "almanac open HOST>Dev>Secret");
        assert_eq!(blocked.first_tag(), Some(StatusTag::Locked));
        // The lock is an Aaru gate only — the real folder is untouched.
        assert!(work.path().join("Secret").join("plan.txt").is_file());

        evaluate(&mut state, "ROOT", "almanac unlock HOST>Dev>Secret");
        assert_eq!(
            respond(&mut state, "vault-password").first_tag(),
            Some(StatusTag::Ok)
        );
        assert!(evaluate(&mut state, "ROOT", "almanac open HOST>Dev>Secret")
            .new_cwd
            .is_some());
    }

    #[test]
    fn duplicate_mount_alias_is_resolved_deterministically_via_the_engine() {
        let dir = tempfile::tempdir().unwrap();
        let work_a = tempfile::tempdir().unwrap();
        let work_b = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        let a = state.host_mount(work_a.path(), Some("Shared")).unwrap();
        let b = state.host_mount(work_b.path(), Some("Shared")).unwrap();
        assert_eq!(a, "Shared");
        assert_eq!(b, "Shared-2");
        let listing = evaluate(&mut state, "ROOT", "almanac mounts");
        assert!(listing.rendered().join("\n").contains("HOST>Shared-2"));
    }

    // -------------------- Phase 5: launcher & process manager --------------------

    #[test]
    fn run_builtin_registers_a_simulated_process_and_opens_its_window() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let outcome = evaluate(&mut state, "ROOT", "almanac run Calculator");
        assert_eq!(outcome.first_tag(), Some(StatusTag::Process));
        assert_eq!(outcome.open_window.as_deref(), Some("calculator"));
        let listed = evaluate(&mut state, "ROOT", "almanac process");
        let text = listed.rendered().join("\n");
        assert!(text.contains("Calculator"));
        assert!(text.contains("AaruApp"));
        assert!(text.contains("[SIM"));
        // parent-child linkage: the built-in is a child of the Almanac launcher (PID 2)
        assert!(text.contains("ppid   2"));
    }

    #[test]
    fn run_game_registers_an_aaru_game_process_with_workload() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        let outcome = evaluate(&mut state, "ROOT", "almanac run Tetris");
        let text = outcome.rendered().join("\n");
        assert!(text.contains("simulated workload:"));
        assert!(evaluate(&mut state, "ROOT", "almanac process")
            .rendered()
            .join("\n")
            .contains("AaruGame"));
    }

    #[test]
    fn run_unknown_application_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac run zzz-not-an-app-9000").first_tag(),
            Some(StatusTag::Error)
        );
    }

    #[test]
    fn terminate_suspend_resume_via_almanac_verbs() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        evaluate(&mut state, "ROOT", "almanac run Snake");
        // Snake gets the first dynamic PID, 3.
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac suspend 3").first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac resume 3").first_tag(),
            Some(StatusTag::Ok)
        );
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac terminate 3").first_tag(),
            Some(StatusTag::Ok)
        );
        // A second terminate on the same PID now fails.
        assert_ne!(
            evaluate(&mut state, "ROOT", "almanac terminate 3").first_tag(),
            Some(StatusTag::Ok)
        );
    }

    #[test]
    fn protected_and_untracked_pids_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac terminate 1").first_tag(),
            Some(StatusTag::Denied)
        );
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac terminate 99999").first_tag(),
            Some(StatusTag::Error)
        );
    }

    #[test]
    fn opening_a_host_file_in_a_path_command_launches_and_tracks_it() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());
        // Use a program guaranteed to exist on the test platform as the "app".
        let app = if cfg!(windows) { "cmd" } else { "sh" };
        let outcome = evaluate(
            &mut state,
            "ROOT",
            &format!("almanac rewrite HOST>Dev>notes.txt in {app}"),
        );
        let text = outcome.rendered().join("\n");
        assert_eq!(outcome.first_tag(), Some(StatusTag::Info));
        assert!(text.contains("[HOST]"));
        // The launched editor shows up in the process table as a host process.
        let processes = evaluate(&mut state, "ROOT", "almanac process")
            .rendered()
            .join("\n");
        assert!(processes.contains("HostApp"));
        // Clean up — terminate every non-protected process we spawned.
        for pid in state.process_list().unwrap().iter().map(|p| p.pid) {
            let _ = state.process_terminate(pid);
        }
    }

    #[test]
    fn reveal_is_host_only() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac reveal Documents").first_tag(),
            Some(StatusTag::Error)
        );
    }

    // -------------------- Phase 6: virtual CPU scheduler --------------------

    #[test]
    fn scheduler_status_reports_the_active_algorithm_and_cores() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let status = evaluate(&mut state, "ROOT", "almanac scheduler");
        assert_eq!(status.first_tag(), Some(StatusTag::Ok));
        let text = status.rendered().join("\n");
        assert!(text.contains("Round Robin"));
        assert!(text.contains("quantum 4 ticks"));
        assert!(text.contains("Core 0:"));
        assert!(text.contains("Core 1:"));
        assert!(text.contains("context switches:"));
        assert!(text.contains("virtual CPU utilization:"));
    }

    #[test]
    fn scheduler_change_switches_algorithm_and_is_reflected_in_status() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let changed = evaluate(&mut state, "ROOT", "almanac scheduler change FCFS");
        assert_eq!(changed.first_tag(), Some(StatusTag::Ok));
        assert!(changed.rendered().join("\n").contains("switched to FCFS"));

        let status = evaluate(&mut state, "ROOT", "almanac scheduler");
        let text = status.rendered().join("\n");
        assert!(text.contains("scheduler: FCFS"));
        assert!(!text.contains("quantum"));

        assert!(
            evaluate(&mut state, "ROOT", "almanac scheduler change nonsense")
                .rendered()
                .join("\n")
                .contains("unknown scheduler")
        );
    }

    #[test]
    fn scheduler_tick_advances_the_simulation_and_dispatches_launched_apps() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        evaluate(&mut state, "ROOT", "almanac run Calculator");
        evaluate(&mut state, "ROOT", "almanac run Snake");
        evaluate(&mut state, "ROOT", "almanac run Tetris");

        let ticked = evaluate(&mut state, "ROOT", "almanac scheduler tick 6");
        assert_eq!(ticked.first_tag(), Some(StatusTag::Ok));
        let text = ticked.rendered().join("\n");
        assert!(text.contains("advanced the virtual CPU by 6"));
        assert!(text.contains("tick 6"));

        // The virtual CPU has two cores, so exactly two of the three launched
        // apps run at once; the third is off-core (READY or WAITING).
        let processes = evaluate(&mut state, "ROOT", "almanac process")
            .rendered()
            .join("\n");
        let running = processes
            .lines()
            .filter(|line| line.contains("AaruApp") || line.contains("AaruGame"))
            .filter(|line| line.contains("Running"))
            .count();
        assert_eq!(running, 2, "only two virtual cores can run at once");
        assert!(
            processes.contains("Ready") || processes.contains("Waiting"),
            "the third simulated process is off-core"
        );
    }

    #[test]
    fn scheduler_commands_require_login() {
        let dir = tempfile::tempdir().unwrap();
        // fresh state, no configure_login
        let mut state = SystemState::fresh(JsonPersistence::new(dir.path().join("state.json")));
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac scheduler").first_tag(),
            Some(StatusTag::Auth)
        );
    }

    // -------------------- Phase 7: simulated memory --------------------

    #[test]
    fn memory_status_reports_the_aaru_ram_swap_and_paging_model() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let status = evaluate(&mut state, "ROOT", "almanac memory");
        assert_eq!(status.first_tag(), Some(StatusTag::Ok));
        let text = status.rendered().join("\n");
        assert!(text.contains("AARU MEMORY"));
        assert!(text.contains("FIFO replacement"));
        assert!(text.contains("RAM: 0 / 4096 MB used"));
        assert!(text.contains("Frames: 0 / 1024 used"));
        assert!(text.contains("Swap: 0 / 4096 MB used"));
        assert!(text.contains("Page faults: 0 · Page hits: 0"));
        assert!(text.contains("independent of Windows host memory"));
    }

    #[test]
    fn launching_and_terminating_a_process_allocates_then_frees_frames() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        evaluate(&mut state, "ROOT", "almanac run Calculator"); // 16 MB → 4 frames
        let after_launch = evaluate(&mut state, "ROOT", "almanac memory")
            .rendered()
            .join("\n");
        assert!(after_launch.contains("Frames: 4 / 1024 used"));
        assert!(after_launch.contains("RAM: 16 / 4096 MB used"));

        // Calculator is the first dynamic PID, 3.
        evaluate(&mut state, "ROOT", "almanac terminate 3");
        let after_kill = evaluate(&mut state, "ROOT", "almanac memory")
            .rendered()
            .join("\n");
        assert!(after_kill.contains("Frames: 0 / 1024 used"));
    }

    #[test]
    fn memory_policy_switches_between_fifo_and_lru() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);

        let changed = evaluate(&mut state, "ROOT", "almanac memory policy LRU");
        assert_eq!(changed.first_tag(), Some(StatusTag::Ok));
        let text = changed.rendered().join("\n");
        assert!(text.contains("policy set to LRU"));
        assert!(text.contains("LRU replacement"));

        assert!(
            evaluate(&mut state, "ROOT", "almanac memory policy nonsense")
                .rendered()
                .join("\n")
                .contains("unknown replacement policy")
        );
    }

    #[test]
    fn memory_command_requires_login() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = SystemState::fresh(JsonPersistence::new(dir.path().join("state.json")));
        assert_eq!(
            evaluate(&mut state, "ROOT", "almanac memory").first_tag(),
            Some(StatusTag::Auth)
        );
    }

    #[test]
    fn run_launches_a_host_shortcut_via_windows_without_tracking_it() {
        let dir = tempfile::tempdir().unwrap();
        let work = tempfile::tempdir().unwrap();
        let mut state = host_state(&dir, work.path());
        std::fs::write(work.path().join("VALORANT.lnk"), b"shortcut").unwrap();

        let before = state.process_list().unwrap().len();
        let outcome = evaluate(&mut state, "ROOT", "almanac run HOST>Dev>VALORANT.lnk");
        assert_eq!(outcome.first_tag(), Some(StatusTag::Process));
        let text = outcome.rendered().join("\n");
        assert!(text.contains("via Windows"));
        assert!(text.contains("not tracked"));
        // A shortcut is handed to the OS default handler — no PCB added.
        assert_eq!(state.process_list().unwrap().len(), before);
        // …and the same works with a bare name from a host cwd.
        let relative = evaluate(&mut state, "HOST>Dev", "almanac run VALORANT.lnk");
        assert_eq!(relative.first_tag(), Some(StatusTag::Process));
    }

    #[test]
    fn logout_requires_a_separate_correct_password() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        let request = evaluate(&mut state, "ROOT", "almanac logout");
        let prompt = request.prompt.unwrap();
        assert_eq!(prompt.message, "Password:");
        assert!(prompt.masked);

        let wrong = respond(&mut state, "wrong-password");
        assert_eq!(wrong.first_tag(), Some(StatusTag::Denied));
        assert!(state.authentication_status().authenticated);

        let request = evaluate(&mut state, "ROOT", "almanac logout");
        assert!(request.prompt.is_some());
        let correct = respond(&mut state, "login-password");
        assert!(matches!(
            correct.system_action,
            Some(SystemAction::LoggedOut)
        ));
        assert!(!state.authentication_status().authenticated);
    }

    #[test]
    fn kill_lapsession_warns_confirms_and_clears_only_aaru_runtime() {
        let dir = tempfile::tempdir().unwrap();
        let mut state = logged_in_state(&dir);
        evaluate(&mut state, "ROOT", "almanac run Calculator");
        let request = evaluate(&mut state, "ROOT", "almanac kill lapsession");
        assert!(request
            .rendered()
            .join("\n")
            .contains("Windows and unrelated"));
        assert_eq!(
            request.prompt.as_ref().map(|prompt| prompt.kind.as_str()),
            Some("kill_lapsession_confirm")
        );

        let cancelled = respond(&mut state, "n");
        assert!(cancelled.system_action.is_none());
        assert!(state.memory_snapshot().unwrap().ram_used_mb > 0);

        evaluate(&mut state, "ROOT", "almanac kill lapsession");
        let confirmed = respond(&mut state, "y");
        assert!(matches!(
            confirmed.system_action,
            Some(SystemAction::Shutdown)
        ));
        assert_eq!(state.memory_snapshot().unwrap().ram_used_mb, 0);
        assert_eq!(state.scheduler_snapshot().unwrap().schedulable_count, 0);
    }
}
