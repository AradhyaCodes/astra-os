//! Thin Tauri IPC boundary. Authentication decisions and filesystem policy
//! remain in Rust's [`SystemState`], never in React.

use crate::almanac::{AlmanacOutcome, CompletionResult, SystemAction};
use crate::error::AaruError;
use crate::filesystem::{DeleteSummary, Permissions, ResourceInfo, SearchResults};
use crate::fs_provider::MountView;
use crate::kernel::SystemConfig;
use crate::security::{AuthenticationStatus, ResourceAuthenticationStatus};
use crate::shell::{HostError, ProcessRunner, StreamEvent, SystemProcessRunner};
use crate::state::{AppState, ResourceSecurityInfo, SystemState};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::{RwLockReadGuard, RwLockWriteGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tauri::{Emitter, Manager};
use zeroize::Zeroize;

#[derive(Debug, Clone, Serialize)]
pub struct BootCheck {
    pub name: &'static str,
    pub detail: String,
    pub ok: bool,
}

#[derive(Debug, Serialize)]
pub struct BootReport {
    pub version: String,
    pub checks: Vec<BootCheck>,
    pub resumed: bool,
    pub resume_session: Option<crate::persistence::ResumeSession>,
}

fn read_system(state: &AppState) -> Result<RwLockReadGuard<'_, SystemState>, AaruError> {
    state
        .0
        .read()
        .map_err(|_| AaruError::Process("Failed to acquire system read lock".to_string()))
}

fn write_system(state: &AppState) -> Result<RwLockWriteGuard<'_, SystemState>, AaruError> {
    state
        .0
        .write()
        .map_err(|_| AaruError::Process("Failed to acquire system write lock".to_string()))
}

#[tauri::command]
pub fn get_system_config() -> Result<SystemConfig, AaruError> {
    Ok(SystemConfig::default())
}

#[tauri::command]
pub fn boot_status(state: tauri::State<'_, AppState>) -> Result<BootReport, AaruError> {
    let mut system = write_system(&state)?;
    let resumed = system.has_resumed_runtime();
    let resume_session = system.take_resume_session();
    Ok(BootReport {
        version: "0.1".to_string(),
        resumed,
        resume_session,
        // Reaching this command means state construction, persistence recovery,
        // host bridge installation, and IPC wiring all succeeded. Values are
        // fixed kernel facts or live initialized subsystem boundaries.
        checks: vec![
            BootCheck {
                name: "Virtual CPU",
                detail: "2 deterministic cores".into(),
                ok: true,
            },
            BootCheck {
                name: "Aaru RAM",
                detail: format!("{} MB simulated", crate::kernel::RAM_MB),
                ok: true,
            },
            BootCheck {
                name: "Virtual filesystem",
                detail: "persistent store mounted".into(),
                ok: true,
            },
            BootCheck {
                name: "Host filesystem bridge",
                detail: "mount router ready".into(),
                ok: true,
            },
            BootCheck {
                name: "Scheduler",
                detail: "runtime initialized".into(),
                ok: true,
            },
            BootCheck {
                name: "Security",
                detail: "Argon2 credential gate ready".into(),
                ok: true,
            },
            BootCheck {
                name: "Almanac",
                detail: "command engine ready".into(),
                ok: true,
            },
            BootCheck {
                name: "Desktop",
                detail: "Tauri IPC connected".into(),
                ok: true,
            },
        ],
    })
}

#[tauri::command]
pub fn lifecycle_hibernate(
    state: tauri::State<'_, AppState>,
    cwd: String,
    ui_session: serde_json::Value,
    almanac_session: serde_json::Value,
) -> Result<(), AaruError> {
    write_system(&state)?.prepare_hibernate(cwd, ui_session, almanac_session)
}

#[tauri::command]
pub fn lifecycle_resume(
    state: tauri::State<'_, AppState>,
) -> Result<Option<crate::persistence::ResumeSession>, AaruError> {
    Ok(write_system(&state)?.resume_runtime())
}

#[tauri::command]
pub fn auth_status(state: tauri::State<'_, AppState>) -> Result<AuthenticationStatus, AaruError> {
    Ok(read_system(&state)?.authentication_status())
}

#[tauri::command]
pub fn configure_login(
    state: tauri::State<'_, AppState>,
    mut password: String,
) -> Result<AuthenticationStatus, AaruError> {
    let result = write_system(&state)?.configure_login(&password);
    password.zeroize();
    result
}

#[tauri::command]
pub fn login(
    state: tauri::State<'_, AppState>,
    mut password: String,
) -> Result<AuthenticationStatus, AaruError> {
    let result = write_system(&state)?.login(&password);
    password.zeroize();
    result
}

#[tauri::command]
pub fn logout(state: tauri::State<'_, AppState>) -> Result<AuthenticationStatus, AaruError> {
    Ok(write_system(&state)?.logout())
}

// ---------------------------------------------------------------------------
// Almanac command engine
// ---------------------------------------------------------------------------

/// Evaluate one raw Almanac line. Custom `almanac …` commands are handled by
/// the Rust engine; anything else is streamed to the host shell.
#[tauri::command]
pub fn almanac_eval(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    cwd: String,
    line: String,
) -> Result<AlmanacOutcome, AaruError> {
    let mut outcome = {
        let mut system = write_system(&state)?;
        let outcome = crate::almanac::evaluate(&mut system, &cwd, &line);
        // The raw line never carries a secret — passwords only ever arrive via
        // `almanac_respond`. Record it (prompt or not) for Up/Down history.
        system.record_command_history(&line);
        outcome
    };
    let host_cwd = host_working_directory(&app, &cwd);
    finish_outcome(&app, &state, &mut outcome, host_cwd);
    Ok(outcome)
}

/// Answer an outstanding interactive prompt (`destroy` confirmation, lock /
/// unlock / logout passwords). The response is never written to history.
#[tauri::command]
pub fn almanac_respond(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    mut response: String,
) -> Result<AlmanacOutcome, AaruError> {
    let mut outcome = {
        let mut system = write_system(&state)?;
        crate::almanac::respond(&mut system, &response)
    };
    response.zeroize();
    finish_outcome(&app, &state, &mut outcome, None);
    Ok(outcome)
}

/// Abandon the current interactive prompt (Esc).
#[tauri::command]
pub fn almanac_cancel_prompt(
    state: tauri::State<'_, AppState>,
) -> Result<AlmanacOutcome, AaruError> {
    let mut system = write_system(&state)?;
    Ok(crate::almanac::cancel(&mut system))
}

/// Case-insensitive tab completion for the current line.
#[tauri::command]
pub fn almanac_complete(
    state: tauri::State<'_, AppState>,
    cwd: String,
    line: String,
) -> Result<CompletionResult, AaruError> {
    let system = read_system(&state)?;
    Ok(crate::almanac::complete(&system, &cwd, &line))
}

/// Persistent command history (oldest first), for Up/Down navigation.
#[tauri::command]
pub fn almanac_history(state: tauri::State<'_, AppState>) -> Result<Vec<String>, AaruError> {
    Ok(read_system(&state)?.command_history().to_vec())
}

// ---------------------------------------------------------------------------
// Host filesystem bridge
// ---------------------------------------------------------------------------

/// Open the OS folder picker (Rust-side — the chosen path never round-trips
/// through untrusted JS as a raw string; it is canonicalised and
/// containment-checked when it becomes a mount).
#[tauri::command]
pub async fn host_pick_directory(app: tauri::AppHandle) -> Result<Option<String>, AaruError> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        app.dialog().file().blocking_pick_folder()
    })
    .await
    .map_err(|error| AaruError::Process(error.to_string()))?;

    Ok(picked
        .and_then(|file| file.into_path().ok())
        .map(|path| path.display().to_string()))
}

/// Approve a directory as a host mount. Validates + canonicalises in Rust.
#[tauri::command]
pub fn host_mount(
    state: tauri::State<'_, AppState>,
    path: String,
    alias: Option<String>,
) -> Result<String, AaruError> {
    write_system(&state)?.host_mount(Path::new(&path), alias.as_deref())
}

#[tauri::command]
pub fn host_unmount(state: tauri::State<'_, AppState>, alias: String) -> Result<(), AaruError> {
    write_system(&state)?.host_unmount(&alias)
}

#[tauri::command]
pub fn host_mounts(state: tauri::State<'_, AppState>) -> Result<Vec<MountView>, AaruError> {
    read_system(&state)?.host_mount_list()
}

/// Authenticate an Aaru lock on a host directory for this session. No terminal
/// verb drives this yet — it is the stable boundary a future host-security
/// panel will call, mirroring `fs_authenticate_resource` for virtual locks.
#[tauri::command]
pub fn host_authenticate(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    mut password: String,
) -> Result<ResourceAuthenticationStatus, AaruError> {
    let result = (|| {
        let mut system = write_system(&state)?;
        match system.route(&cwd, &path)? {
            crate::fs_provider::AaruLocation::Host { mount, relative } => {
                system.host_authenticate(&mount, &relative, &password)
            }
            _ => Err(AaruError::InvalidPath("not a host path".to_string())),
        }
    })();
    password.zeroize();
    result
}

/// Resolve a real host working directory that mirrors the Aaru cwd under the
/// app data directory. Aaru names are validated (no path separators, none of
/// `< > : " / \ | ? *`), so they are safe path components. Returns `None` when
/// no data dir is available.
fn host_working_directory(app: &tauri::AppHandle, aaru_cwd: &str) -> Option<PathBuf> {
    let mut base = app.path().app_data_dir().ok()?;
    base.push("host-workspace");
    for component in aaru_cwd.split('>') {
        let component = component.trim();
        if component.is_empty() || component == "ROOT" || component == "." {
            continue;
        }
        if component == ".." {
            base.pop();
            continue;
        }
        base.push(component);
    }
    std::fs::create_dir_all(&base).ok()?;
    Some(base)
}

// ---------------------------------------------------------------------------
// Process manager
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn process_list(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<crate::process::PcbView>, AaruError> {
    write_system(&state)?.process_list()
}

#[tauri::command]
pub fn process_terminate(
    state: tauri::State<'_, AppState>,
    pid: u32,
) -> Result<crate::process::PcbView, AaruError> {
    write_system(&state)?.process_terminate(pid)
}

#[tauri::command]
pub fn process_suspend(
    state: tauri::State<'_, AppState>,
    pid: u32,
) -> Result<crate::process::PcbView, AaruError> {
    write_system(&state)?.process_suspend(pid)
}

#[tauri::command]
pub fn process_resume(
    state: tauri::State<'_, AppState>,
    pid: u32,
) -> Result<crate::process::PcbView, AaruError> {
    write_system(&state)?.process_resume(pid)
}

// ---------------------------------------------------------------------------
// Virtual CPU scheduler (Phase 6)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn scheduler_status(
    state: tauri::State<'_, AppState>,
) -> Result<crate::scheduler::SchedulerSnapshot, AaruError> {
    read_system(&state)?.scheduler_snapshot()
}

#[tauri::command]
pub fn scheduler_set_algorithm(
    state: tauri::State<'_, AppState>,
    algorithm: String,
) -> Result<crate::scheduler::SchedulerSnapshot, AaruError> {
    let algorithm = crate::scheduler::parse_algorithm(&algorithm)?;
    write_system(&state)?.scheduler_set_algorithm(algorithm)
}

/// Manually step the deterministic simulation (mirrors `almanac scheduler tick`).
#[tauri::command]
pub fn scheduler_tick(
    state: tauri::State<'_, AppState>,
    count: Option<u64>,
) -> Result<crate::scheduler::SchedulerSnapshot, AaruError> {
    let mut system = write_system(&state)?;
    system.require_authentication()?;
    system.scheduler_tick(count.unwrap_or(1).clamp(1, 100_000));
    system.scheduler_snapshot()
}

// ---------------------------------------------------------------------------
// Simulated memory subsystem (Phase 7)
// ---------------------------------------------------------------------------

#[tauri::command]
pub fn memory_status(
    state: tauri::State<'_, AppState>,
) -> Result<crate::memory::MemorySnapshot, AaruError> {
    read_system(&state)?.memory_snapshot()
}

#[tauri::command]
pub fn memory_set_policy(
    state: tauri::State<'_, AppState>,
    policy: String,
) -> Result<crate::memory::MemorySnapshot, AaruError> {
    let policy = crate::memory::parse_replacement_policy(&policy).ok_or_else(|| {
        AaruError::InvalidArgument(format!(
            "unknown replacement policy '{policy}' — use FIFO or LRU"
        ))
    })?;
    write_system(&state)?.memory_set_policy(policy)
}

fn render_host_command(command: &crate::shell::HostCommand) -> String {
    if command.args.is_empty() {
        command.program.clone()
    } else {
        format!("{} {}", command.program, command.args.join(" "))
    }
}

/// Carry out the side effects an [`AlmanacOutcome`] asks for: spawn + stream a
/// host process (registering it in the Aaru process table), open a host file
/// with its default application, or perform a whole-session action.
fn finish_outcome(
    app: &tauri::AppHandle,
    state: &AppState,
    outcome: &mut AlmanacOutcome,
    host_cwd: Option<PathBuf>,
) {
    // Open a real host file with its default Windows app, or a verified web
    // fallback for a direct application shortcut.
    if let Some(launch) = outcome.launch.take() {
        if let Some(target) = launch.path {
            use tauri_plugin_opener::OpenerExt;
            match launch.app.as_str() {
                "$default" => {
                    let _ = app.opener().open_path(target, None::<&str>);
                }
                "$url" => {
                    let _ = app.opener().open_url(target, None::<&str>);
                }
                _ => {}
            }
        }
    }

    if let Some(mut command) = outcome.shell.take() {
        command.cwd = host_cwd;
        let id = format!(
            "proc-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
        let channel = format!("almanac://proc/{id}");
        let program = command.program.clone();
        let view_program = program.clone();
        let display = render_host_command(&command);
        let app_handle = app.clone();
        let app_state = state.clone();
        std::thread::spawn(move || {
            let mut aaru_pid: Option<crate::process::Pid> = None;
            let mut emit = |event: StreamEvent| {
                match &event {
                    StreamEvent::Started { pid } => {
                        if let Ok(mut system) = app_state.0.write() {
                            aaru_pid =
                                Some(system.register_shell_process(&program, &display, *pid).pid);
                        }
                    }
                    StreamEvent::Exit { .. } => {
                        if let (Some(pid), Ok(mut system)) = (aaru_pid, app_state.0.write()) {
                            system.mark_shell_process_exited(pid);
                        }
                    }
                    _ => {}
                }
                let _ = app_handle.emit(&channel, event);
            };
            if let Err(error) = SystemProcessRunner.run(&command, &mut emit) {
                let not_found = matches!(error, HostError::NotFound(_));
                emit(StreamEvent::Error {
                    message: error.to_string(),
                    not_found,
                });
            }
        });
        outcome.process = Some(crate::almanac::ProcessView {
            id,
            program: view_program,
        });
    }

    match outcome.system_action {
        Some(SystemAction::Shutdown) => {
            let app_handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                app_handle.exit(0);
            });
        }
        Some(SystemAction::Restart) => {
            let app_handle = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(200));
                app_handle.restart();
            });
        }
        // `Hibernate` and `LoggedOut` are handled entirely in the UI for now.
        _ => {}
    }
}

#[tauri::command]
pub fn fs_root(state: tauri::State<'_, AppState>) -> Result<ResourceInfo, AaruError> {
    read_system(&state)?.root()
}

#[tauri::command]
pub fn fs_resolve_path(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<u64, AaruError> {
    read_system(&state)?.resolve_path(&cwd, &path)
}

#[tauri::command]
pub fn fs_open_directory(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<ResourceInfo, AaruError> {
    read_system(&state)?.open_directory(&cwd, &path)
}

#[tauri::command]
pub fn fs_parent_directory(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<ResourceInfo, AaruError> {
    read_system(&state)?.parent_directory(&cwd, &path)
}

#[tauri::command]
pub fn fs_list(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<Vec<ResourceInfo>, AaruError> {
    read_system(&state)?.list_directory(&cwd, &path)
}

#[tauri::command]
pub fn fs_create_directory(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.create_directory(&cwd, &path)
}

#[tauri::command]
pub fn fs_create_file(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    content: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.create_file(&cwd, &path, &content)
}

#[tauri::command]
pub fn fs_read_file(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<String, AaruError> {
    read_system(&state)?.read_file(&cwd, &path)
}

#[tauri::command]
pub fn fs_write_file(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    content: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.write_file(&cwd, &path, &content)
}

#[tauri::command]
pub fn fs_create_tree(
    state: tauri::State<'_, AppState>,
    cwd: String,
    expression: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.create_tree(&cwd, &expression)
}

#[tauri::command]
pub fn fs_rename(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    new_name: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.rename(&cwd, &path, &new_name)
}

#[tauri::command]
pub fn fs_move(
    state: tauri::State<'_, AppState>,
    cwd: String,
    source_path: String,
    destination_directory: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.move_resource(&cwd, &source_path, &destination_directory)
}

#[tauri::command]
pub fn fs_copy(
    state: tauri::State<'_, AppState>,
    cwd: String,
    source_path: String,
    destination_directory: String,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.copy_resource(&cwd, &source_path, &destination_directory)
}

#[tauri::command]
pub fn fs_delete_preview(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<DeleteSummary, AaruError> {
    read_system(&state)?.delete_preview(&cwd, &path)
}

#[tauri::command]
pub fn fs_delete(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<DeleteSummary, AaruError> {
    write_system(&state)?.delete_recursive(&cwd, &path)
}

#[tauri::command]
pub fn fs_search(
    state: tauri::State<'_, AppState>,
    cwd: String,
    start_path: String,
    query: String,
    skip_inaccessible: bool,
) -> Result<SearchResults, AaruError> {
    let _ = skip_inaccessible;
    read_system(&state)?.search(&cwd, &start_path, &query)
}

#[tauri::command]
pub fn fs_inspect(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<ResourceInfo, AaruError> {
    read_system(&state)?.inspect(&cwd, &path)
}

#[tauri::command]
pub fn fs_set_permissions(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    permissions: Permissions,
) -> Result<ResourceInfo, AaruError> {
    write_system(&state)?.set_permissions(&cwd, &path, permissions)
}

#[tauri::command]
pub fn fs_lock(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    mut password: String,
) -> Result<ResourceSecurityInfo, AaruError> {
    let result = write_system(&state)?.lock_resource(&cwd, &path, &password);
    password.zeroize();
    result
}

#[tauri::command]
pub fn fs_authenticate_resource(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    mut password: String,
) -> Result<ResourceAuthenticationStatus, AaruError> {
    let result = write_system(&state)?.authenticate_resource(&cwd, &path, &password);
    password.zeroize();
    result
}

#[tauri::command]
pub fn fs_unlock(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
    mut password: String,
) -> Result<ResourceSecurityInfo, AaruError> {
    let result = write_system(&state)?.unlock_resource(&cwd, &path, &password);
    password.zeroize();
    result
}

#[tauri::command]
pub fn fs_security_info(
    state: tauri::State<'_, AppState>,
    cwd: String,
    path: String,
) -> Result<ResourceSecurityInfo, AaruError> {
    read_system(&state)?.resource_security_info(&cwd, &path)
}

/// The host applications Aaru knows how to launch, and whether each is
/// installed on this machine. Pure detection — no processes are started.
#[tauri::command]
pub fn host_apps() -> Vec<crate::process::HostAppInfo> {
    crate::process::list_host_apps()
}
