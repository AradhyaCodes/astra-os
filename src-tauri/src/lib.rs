//! Astra OS — Library entry point
//!
//! Declares all kernel submodules and wires up the Tauri application builder
//! with IPC command handlers and plugins.

// ---------------------------------------------------------------------------
// Submodule declarations
// ---------------------------------------------------------------------------

pub mod error;
pub mod kernel;

pub mod commands;

pub mod almanac;
pub mod fs_provider;

// Subsystem stubs — will be fleshed out in subsequent phases
pub mod filesystem;
pub mod memory;
pub mod persistence;
pub mod process;
pub mod scheduler;
pub mod security;
pub mod shell;
pub mod state;

// ---------------------------------------------------------------------------
// Tauri application entry point
// ---------------------------------------------------------------------------

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    use tauri::Manager;

    // Initialise env_logger — reads RUST_LOG env var for log level.
    // Default to "info" in debug builds.
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    log::info!("Astra OS v0.1 starting…");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir()?;
            let state_path = app_data_dir.join("state.json");
            if !state_path.exists() {
                if let Some(parent) = app_data_dir.parent() {
                    let legacy_state = parent.join("com.aaru.os").join("state.json");
                    if legacy_state.exists() {
                        std::fs::create_dir_all(&app_data_dir)?;
                        std::fs::copy(&legacy_state, &state_path)?;
                        log::info!("migrated the Aaru-OS profile into Astra OS");
                    }
                }
            }
            let persistence = crate::persistence::JsonPersistence::new(state_path);
            let report = persistence.load_recovering()?;
            if let Some(notice) = &report.recovery_notice {
                log::warn!("Astra OS persistence recovery: {notice}");
            }

            let mut system = crate::state::SystemState::from_snapshot(report.snapshot, persistence);
            // Resolve the real Windows user directories with OS APIs — no
            // hardcoded usernames — and (re)install the default host mounts.
            let resolver = app.path();
            system.install_host_dirs(crate::fs_provider::host::HostDirs {
                desktop: resolver.desktop_dir().ok(),
                documents: resolver.document_dir().ok(),
                downloads: resolver.download_dir().ok(),
                home: resolver.home_dir().ok(),
                // The all-users Desktop (system-wide app shortcuts). Windows
                // shows it merged with the user Desktop; we expose it as its
                // own `HOST>PublicDesktop` mount.
                public_desktop: std::env::var_os("PUBLIC")
                    .map(|public| std::path::PathBuf::from(public).join("Desktop")),
            });

            app.manage(crate::state::AppState(std::sync::Arc::new(
                std::sync::RwLock::new(system),
            )));

            // Phase 6 — drive the virtual CPU scheduler on a fixed cadence so
            // the simulation advances on its own, independent of the frontend.
            // The tick itself is fully deterministic; this thread only decides
            // *when* to call it. `almanac scheduler tick` steps it manually too.
            let scheduler_state = app.state::<crate::state::AppState>().inner().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(500));
                if let Ok(mut system) = scheduler_state.0.write() {
                    system.scheduler_tick(1);
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::get_system_config,
            commands::boot_status,
            commands::lifecycle_hibernate,
            commands::lifecycle_resume,
            commands::auth_status,
            commands::configure_login,
            commands::login,
            commands::logout,
            commands::almanac_eval,
            commands::almanac_respond,
            commands::almanac_cancel_prompt,
            commands::almanac_complete,
            commands::almanac_history,
            commands::host_pick_directory,
            commands::host_mount,
            commands::host_unmount,
            commands::host_mounts,
            commands::host_authenticate,
            commands::process_list,
            commands::process_terminate,
            commands::process_suspend,
            commands::process_resume,
            commands::scheduler_status,
            commands::scheduler_set_algorithm,
            commands::scheduler_tick,
            commands::memory_status,
            commands::memory_set_policy,
            commands::fs_root,
            commands::fs_resolve_path,
            commands::fs_open_directory,
            commands::fs_parent_directory,
            commands::fs_list,
            commands::fs_create_directory,
            commands::fs_create_file,
            commands::fs_read_file,
            commands::fs_write_file,
            commands::fs_create_tree,
            commands::fs_rename,
            commands::fs_move,
            commands::fs_copy,
            commands::fs_delete_preview,
            commands::fs_delete,
            commands::fs_search,
            commands::fs_inspect,
            commands::fs_set_permissions,
            commands::fs_lock,
            commands::fs_authenticate_resource,
            commands::fs_unlock,
            commands::fs_security_info,
            commands::host_apps,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Astra OS");
}
