//! Safe resolution of real, installed host applications.
//!
//! Resolution order (no full-disk scanning, no hardcoded user paths):
//! 1. a small table of **configured aliases** (`VSCode` → `code` / `Code.exe`),
//! 2. **`PATH` lookup** honouring `PATHEXT`,
//! 3. Windows **`App Paths`** registry keys
//!    (`…\CurrentVersion\App Paths\<exe>`), HKCU then HKLM,
//! 4. a few **standard per-user / per-machine install roots** derived from
//!    environment variables (`%LOCALAPPDATA%\Programs`, `%ProgramFiles%`,
//!    `%ProgramFiles(x86)%`) joined with a relative path the alias declares —
//!    still no scanning and no literal user names.
//!
//! A caller asks for either a known alias or a bare command name; anything that
//! does not resolve is reported as not installed.

use serde::Serialize;
use std::path::{Path, PathBuf};

/// One configured application alias.
struct HostAppAlias {
    /// Display / lookup name.
    alias: &'static str,
    /// Executable base names to try, in order.
    candidates: &'static [&'static str],
    /// `App Paths` subkey to consult (usually `<exe>.exe`).
    app_paths_key: Option<&'static str>,
    /// Relative `<subdir>/<exe>` paths to probe under the standard install
    /// roots (see module docs) when `PATH` / `App Paths` come up empty.
    install_paths: &'static [&'static str],
}

const ALIASES: &[HostAppAlias] = &[
    HostAppAlias {
        alias: "ChatGPT",
        candidates: &["ChatGPT.exe", "chatgpt.exe"],
        app_paths_key: Some("ChatGPT.exe"),
        install_paths: &["ChatGPT/ChatGPT.exe"],
    },
    HostAppAlias {
        alias: "Claude",
        candidates: &["Claude.exe", "claude.exe"],
        app_paths_key: Some("Claude.exe"),
        install_paths: &["Claude/Claude.exe", "AnthropicClaude/claude.exe"],
    },
    HostAppAlias {
        alias: "Brave",
        candidates: &["brave.exe", "brave"],
        app_paths_key: Some("brave.exe"),
        install_paths: &["BraveSoftware/Brave-Browser/Application/brave.exe"],
    },
    HostAppAlias {
        alias: "Chrome",
        candidates: &["chrome.exe", "chrome"],
        app_paths_key: Some("chrome.exe"),
        install_paths: &["Google/Chrome/Application/chrome.exe"],
    },
    HostAppAlias {
        alias: "VSCode",
        candidates: &["code", "code.cmd", "code.exe"],
        app_paths_key: Some("Code.exe"),
        install_paths: &[
            "Microsoft VS Code/Code.exe",
            "Microsoft VS Code/bin/code.cmd",
        ],
    },
    HostAppAlias {
        alias: "Antigravity",
        candidates: &["antigravity", "antigravity.cmd", "Antigravity.exe"],
        app_paths_key: Some("Antigravity.exe"),
        install_paths: &[
            "Antigravity/Antigravity.exe",
            "Antigravity IDE/Antigravity IDE.exe",
            "Antigravity IDE/bin/antigravity-ide.cmd",
        ],
    },
    HostAppAlias {
        alias: "Notepad",
        candidates: &["notepad.exe", "notepad"],
        app_paths_key: Some("notepad.exe"),
        install_paths: &[],
    },
    HostAppAlias {
        alias: "Explorer",
        candidates: &["explorer.exe", "explorer"],
        app_paths_key: None,
        install_paths: &[],
    },
    HostAppAlias {
        alias: "Notepad++",
        candidates: &["notepad++.exe"],
        app_paths_key: Some("notepad++.exe"),
        install_paths: &[],
    },
    HostAppAlias {
        alias: "SublimeText",
        candidates: &["subl", "sublime_text.exe"],
        app_paths_key: Some("sublime_text.exe"),
        install_paths: &[],
    },
];

/// Known Microsoft Store / MSIX packages for configured aliases.
///
/// Launch target is `shell:AppsFolder\<package_family>!<app_id>` handed to the
/// Windows shell. `package_family` is the stable *PackageFamilyName* (name plus
/// publisher-id hash); `app_id` is the `Application Id` from the package
/// manifest.
struct StoreApp {
    alias: &'static str,
    package_family: &'static str,
    app_id: &'static str,
}

const STORE_APPS: &[StoreApp] = &[
    StoreApp {
        alias: "Claude",
        package_family: "Claude_pzs8sxrjxfjjc",
        app_id: "Claude",
    },
    StoreApp {
        alias: "ChatGPT",
        package_family: "OpenAI.ChatGPT-Desktop_2p2nqsd0c76g0",
        app_id: "App",
    },
];

/// What `resolve_host_app` produced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostAppResolution {
    /// A known alias / command resolved to a real executable.
    Found { display: String, program: PathBuf },
    /// An installed Microsoft Store / MSIX app. Launch it through the Windows
    /// shell (`explorer shell:AppsFolder\<aumid>`), not as a plain executable.
    StoreApp { display: String, aumid: String },
    /// A known alias, but the executable is not installed.
    NotInstalled { alias: String },
    /// Not a configured alias and not on `PATH` — the caller may treat it as a
    /// non-application (e.g. fall back to a built-in Astra app).
    Unknown,
}

/// Map a friendly shortcut (`vsc`, `google`, `antigravity`, …) to a configured
/// alias so it can be used anywhere an app name is accepted — `almanac run`,
/// `open … in <app>`, `rewrite … in <app>`. Unknown names pass through.
pub fn canonical_app_alias(name: &str) -> &str {
    match name.trim().to_ascii_lowercase().as_str() {
        "vsc" | "vscode" | "code" => "VSCode",
        "google" | "chrome" => "Chrome",
        "antigravity" | "ag" => "Antigravity",
        "chatgpt" | "gpt" => "ChatGPT",
        "claude" => "Claude",
        "brave" => "Brave",
        "notepad" => "Notepad",
        "notepad++" | "npp" => "Notepad++",
        "sublime" | "subl" | "sublimetext" => "SublimeText",
        "explorer" | "files" => "Explorer",
        _ => name.trim(),
    }
}

/// Resolve `request` (an alias like `VSCode`, a shortcut like `vsc`, or a bare
/// command like `code`).
pub fn resolve_host_app(request: &str) -> HostAppResolution {
    let request = canonical_app_alias(request);
    if request.is_empty() {
        return HostAppResolution::Unknown;
    }

    if let Some(entry) = ALIASES
        .iter()
        .find(|entry| entry.alias.eq_ignore_ascii_case(request))
    {
        for candidate in entry.candidates {
            if let Some(program) = lookup_command(candidate) {
                return HostAppResolution::Found {
                    display: entry.alias.to_string(),
                    program,
                };
            }
        }
        if let Some(key) = entry.app_paths_key {
            if let Some(program) = app_paths_lookup(key) {
                return HostAppResolution::Found {
                    display: entry.alias.to_string(),
                    program,
                };
            }
        }
        if let Some(program) = well_known_dir_lookup(entry.install_paths) {
            return HostAppResolution::Found {
                display: entry.alias.to_string(),
                program,
            };
        }
        if let Some(store) = STORE_APPS
            .iter()
            .find(|store| store.alias.eq_ignore_ascii_case(entry.alias))
        {
            if store_package_installed(store.package_family) {
                return HostAppResolution::StoreApp {
                    display: entry.alias.to_string(),
                    aumid: format!("{}!{}", store.package_family, store.app_id),
                };
            }
        }
        return HostAppResolution::NotInstalled {
            alias: entry.alias.to_string(),
        };
    }

    // Not an alias: accept it only if it is a real command on PATH / App Paths.
    if let Some(program) = lookup_command(request) {
        return HostAppResolution::Found {
            display: request.to_string(),
            program,
        };
    }
    if let Some(program) = app_paths_lookup(&ensure_exe(request)) {
        return HostAppResolution::Found {
            display: request.to_string(),
            program,
        };
    }
    HostAppResolution::Unknown
}

/// Names of the configured aliases (for `almanac run` help / listings).
pub fn alias_names() -> Vec<&'static str> {
    ALIASES.iter().map(|entry| entry.alias).collect()
}

/// One host application Astra can launch, plus whether it is installed here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HostAppInfo {
    /// Alias / display name — also the `almanac run <name>` argument.
    pub name: String,
    /// Resolved to a real executable or an installed Store package.
    pub installed: bool,
    /// Installed as a Microsoft Store / MSIX package rather than a plain exe.
    pub store_app: bool,
}

/// Probe every configured alias and report which are installed on this machine.
/// Pure detection — nothing is launched. Installed apps sort first.
pub fn list_host_apps() -> Vec<HostAppInfo> {
    let mut apps: Vec<HostAppInfo> = ALIASES
        .iter()
        .map(|entry| {
            let (installed, store_app) = match resolve_host_app(entry.alias) {
                HostAppResolution::Found { .. } => (true, false),
                HostAppResolution::StoreApp { .. } => (true, true),
                HostAppResolution::NotInstalled { .. } | HostAppResolution::Unknown => {
                    (false, false)
                }
            };
            HostAppInfo {
                name: entry.alias.to_string(),
                installed,
                store_app,
            }
        })
        .collect();
    apps.sort_by(|a, b| {
        b.installed
            .cmp(&a.installed)
            .then_with(|| a.name.cmp(&b.name))
    });
    apps
}

fn ensure_exe(name: &str) -> String {
    if name.to_ascii_lowercase().ends_with(".exe") {
        name.to_string()
    } else {
        format!("{name}.exe")
    }
}

/// Probe standard install roots for any of `relative` (`<subdir>/<exe>`).
///
/// Roots come from environment variables only — `%LOCALAPPDATA%\Programs`
/// (per-user installs), `%ProgramFiles%` and `%ProgramFiles(x86)%` (machine
/// installs). No directory walking: each candidate is a single `is_file` check.
fn well_known_dir_lookup(relative: &[&str]) -> Option<PathBuf> {
    if relative.is_empty() {
        return None;
    }

    let mut roots: Vec<PathBuf> = Vec::new();
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        roots.push(Path::new(&local).join("Programs"));
    }
    for var in ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"] {
        if let Some(value) = std::env::var_os(var) {
            roots.push(PathBuf::from(value));
        }
    }

    for root in roots {
        for rel in relative {
            let candidate = root.join(rel);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Is an MSIX / Store package installed for the current user? Checked without
/// any privileged access — every installed package has a per-user data
/// directory at `%LOCALAPPDATA%\Packages\<package_family_name>`.
fn store_package_installed(package_family: &str) -> bool {
    std::env::var_os("LOCALAPPDATA")
        .map(|local| {
            Path::new(&local)
                .join("Packages")
                .join(package_family)
                .is_dir()
        })
        .unwrap_or(false)
}

/// `PATH` lookup honouring `PATHEXT` on Windows. Returns an absolute path.
fn lookup_command(name: &str) -> Option<PathBuf> {
    // An explicit path was given.
    let direct = Path::new(name);
    if direct.components().count() > 1 || direct.is_absolute() {
        return direct.is_file().then(|| direct.to_path_buf());
    }

    let path_var = std::env::var_os("PATH")?;
    let has_ext = Path::new(name).extension().is_some();
    let exts: Vec<String> = if has_ext {
        vec![String::new()]
    } else {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_string())
            .split(';')
            .map(|ext| ext.trim().to_string())
            .filter(|ext| !ext.is_empty())
            .collect()
    };

    for dir in std::env::split_paths(&path_var) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(windows)]
fn app_paths_lookup(exe: &str) -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;

    let subkey = format!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\App Paths\\{exe}");
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        if let Ok(key) = RegKey::predef(root).open_subkey(&subkey) {
            let raw: Result<String, _> = key.get_value("");
            if let Ok(raw) = raw {
                let trimmed = raw.trim().trim_matches('"');
                let path = PathBuf::from(trimmed);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
    }
    None
}

#[cfg(not(windows))]
fn app_paths_lookup(_exe: &str) -> Option<PathBuf> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_application_resolves_to_unknown() {
        assert_eq!(
            resolve_host_app("definitely-not-installed-xyz"),
            HostAppResolution::Unknown
        );
    }

    #[test]
    fn a_known_alias_without_the_binary_reports_not_installed() {
        // `SublimeText` is very unlikely to be on a CI PATH / App Paths.
        match resolve_host_app("SublimeText") {
            HostAppResolution::NotInstalled { alias } => assert_eq!(alias, "SublimeText"),
            HostAppResolution::Found { .. } => { /* installed on this machine — fine */ }
            HostAppResolution::StoreApp { .. } => panic!("SublimeText is not a Store app"),
            HostAppResolution::Unknown => panic!("SublimeText should be a known alias"),
        }
    }

    #[test]
    fn store_backed_aliases_never_resolve_to_unknown() {
        // Claude / ChatGPT ship as MSIX packages on many machines: even with no
        // executable on PATH they must resolve to a real app or a Store app or
        // a clean "not installed", never to `Unknown` (which would fall through
        // to a built-in / host-shell guess).
        for name in ["claude", "ChatGPT"] {
            assert!(
                !matches!(resolve_host_app(name), HostAppResolution::Unknown),
                "{name} should be a known alias"
            );
        }
    }

    #[test]
    fn store_app_package_probe_is_localappdata_relative() {
        assert!(!store_package_installed(
            "Definitely.NotInstalled_000000000000"
        ));
    }

    #[test]
    fn desktop_shortcuts_are_registered_case_insensitively() {
        for name in ["chatgpt", "CLAUDE", "Brave", "chrome"] {
            assert!(!matches!(
                resolve_host_app(name),
                HostAppResolution::Unknown
            ));
        }
    }

    #[test]
    fn friendly_shortcuts_map_to_canonical_aliases() {
        assert_eq!(canonical_app_alias("vsc"), "VSCode");
        assert_eq!(canonical_app_alias("VSC"), "VSCode");
        assert_eq!(canonical_app_alias(" code "), "VSCode");
        assert_eq!(canonical_app_alias("google"), "Chrome");
        assert_eq!(canonical_app_alias("antigravity"), "Antigravity");
        assert_eq!(canonical_app_alias("subl"), "SublimeText");
        // Unknown names pass through, trimmed.
        assert_eq!(canonical_app_alias("  mspaint "), "mspaint");
        // `vsc` must now resolve as a known alias, not Unknown.
        assert!(!matches!(
            resolve_host_app("vsc"),
            HostAppResolution::Unknown
        ));
    }

    #[test]
    fn a_command_on_path_resolves() {
        // `cmd` / `sh` is always present on the test platform.
        let probe = if cfg!(windows) { "cmd" } else { "sh" };
        assert!(matches!(
            resolve_host_app(probe),
            HostAppResolution::Found { .. }
        ));
    }
}
