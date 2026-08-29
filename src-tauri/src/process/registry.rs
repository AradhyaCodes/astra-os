//! Registry of built-in Astra applications and games.
//!
//! These become *simulated* processes (`ASTRA_APP` / `ASTRA_GAME`). Some are
//! backed by a real Tauri window (`window: Some(app_id)`); the rest are
//! process-only for now and will grow UIs / simulated memory allocation in a
//! later phase.

use super::Priority;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltinKind {
    App,
    Game,
}

#[derive(Debug, Clone, Copy)]
pub struct AstraAppDef {
    /// Lookup key, matched case-insensitively by `almanac run`.
    pub key: &'static str,
    pub name: &'static str,
    pub kind: BuiltinKind,
    pub priority: Priority,
    /// Tauri window `AppId` to open, when the app has a real UI.
    pub window: Option<&'static str>,
    pub sim_cpu_pct: f64,
    pub sim_mem_mb: f64,
    pub workload: &'static str,
}

const BUILTINS: &[AstraAppDef] = &[
    AstraAppDef {
        key: "almanac",
        name: "Almanac",
        kind: BuiltinKind::App,
        priority: Priority::High,
        window: Some("terminal"),
        sim_cpu_pct: 2.0,
        sim_mem_mb: 40.0,
        workload: "interactive command shell",
    },
    AstraAppDef {
        key: "terminal",
        name: "Terminal",
        kind: BuiltinKind::App,
        priority: Priority::Normal,
        window: Some("almanac"),
        sim_cpu_pct: 2.0,
        sim_mem_mb: 36.0,
        workload: "terminal session",
    },
    AstraAppDef {
        key: "taskmanager",
        name: "TaskManager",
        kind: BuiltinKind::App,
        priority: Priority::High,
        window: Some("taskmanager"),
        sim_cpu_pct: 3.0,
        sim_mem_mb: 44.0,
        workload: "process table polling",
    },
    AstraAppDef {
        key: "settings",
        name: "Settings",
        kind: BuiltinKind::App,
        priority: Priority::Normal,
        window: Some("settings"),
        sim_cpu_pct: 1.0,
        sim_mem_mb: 32.0,
        workload: "settings UI",
    },
    AstraAppDef {
        key: "calculator",
        name: "Calculator",
        kind: BuiltinKind::App,
        priority: Priority::Normal,
        window: Some("calculator"),
        sim_cpu_pct: 1.0,
        sim_mem_mb: 22.0,
        workload: "arithmetic UI",
    },
    AstraAppDef {
        key: "texteditor",
        name: "TextEditor",
        kind: BuiltinKind::App,
        priority: Priority::Normal,
        window: Some("texteditor"),
        sim_cpu_pct: 2.0,
        sim_mem_mb: 30.0,
        workload: "text buffer editing",
    },
    AstraAppDef {
        key: "imageviewer",
        name: "ImageViewer",
        kind: BuiltinKind::App,
        priority: Priority::Normal,
        window: Some("imageviewer"),
        sim_cpu_pct: 2.0,
        sim_mem_mb: 34.0,
        workload: "raster image decode + display",
    },
    // ---- games ----
    AstraAppDef {
        key: "snake",
        name: "Snake",
        kind: BuiltinKind::Game,
        priority: Priority::Normal,
        window: Some("app-shell"),
        sim_cpu_pct: 8.0,
        sim_mem_mb: 48.0,
        workload: "grid game loop @ 15 ticks/s",
    },
    AstraAppDef {
        key: "pong",
        name: "Pong",
        kind: BuiltinKind::Game,
        priority: Priority::Normal,
        window: Some("app-shell"),
        sim_cpu_pct: 10.0,
        sim_mem_mb: 46.0,
        workload: "physics + render loop @ 60fps",
    },
    AstraAppDef {
        key: "minesweeper",
        name: "Minesweeper",
        kind: BuiltinKind::Game,
        priority: Priority::Normal,
        window: Some("app-shell"),
        sim_cpu_pct: 4.0,
        sim_mem_mb: 42.0,
        workload: "board solver + flood fill",
    },
    AstraAppDef {
        key: "tetris",
        name: "Tetris",
        kind: BuiltinKind::Game,
        priority: Priority::Normal,
        window: Some("app-shell"),
        sim_cpu_pct: 9.0,
        sim_mem_mb: 50.0,
        workload: "gravity loop + line-clear @ 60fps",
    },
];

pub fn builtins() -> &'static [AstraAppDef] {
    BUILTINS
}

/// Case-insensitive lookup by key or display name.
pub fn find_builtin(query: &str) -> Option<&'static AstraAppDef> {
    let query = query.trim().to_ascii_lowercase();
    BUILTINS
        .iter()
        .find(|def| def.key == query || def.name.to_ascii_lowercase() == query)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_builtin_is_registered() {
        for name in [
            "Almanac",
            "Terminal",
            "TaskManager",
            "Settings",
            "Calculator",
            "TextEditor",
            "ImageViewer",
            "Snake",
            "Pong",
            "Minesweeper",
            "Tetris",
        ] {
            assert!(find_builtin(name).is_some(), "missing builtin {name}");
        }
        assert!(find_builtin("CALCULATOR").is_some());
        assert!(find_builtin("nope").is_none());
    }

    #[test]
    fn games_expose_workload_metadata() {
        let snake = find_builtin("Snake").unwrap();
        assert_eq!(snake.kind, BuiltinKind::Game);
        assert!(snake.sim_mem_mb > 0.0);
        assert!(!snake.workload.is_empty());
    }
}
