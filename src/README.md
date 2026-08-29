# `src/` — Astra OS desktop (React)

The **view and interaction layer only**. Every system decision — auth,
filesystem policy, process/scheduler/memory state, host access — is made in the
Rust core (`../src-tauri`) and reached through Tauri `invoke` calls. Components
here render backend state and forward user intent; they never hold system
truth.

## Layout

| Path | Responsibility |
|---|---|
| `almanac/` | The Almanac console: input handling, history, streamed output, prompt flow. |
| `applications/` | Built-in app windows — `SystemInfo`, `TaskManager`, `SecurityPanel`, `HostApps` (Windows Apps), plus `DesktopApps` (Settings / Calculator / Text Editor / Image Viewer) and `AppShell`. |
| `desktop/` | `Desktop` (icons, host-app tiles, hint panel, window host), `Taskbar` (Start menu, tray, running-window buttons). |
| `components/` | Shared primitives — `Window` chrome, `AppIcon` icon set. |
| `stores/` | `windowStore` — a Zustand store for open windows, focus, z-order, session restore. |
| `hooks/` | Cross-cutting React hooks. |
| `lifecycle/` | Boot screen and startup sequencing. |
| `security/` | The `AuthGate` that gates the desktop behind the profile password. |
| `types/` | Hand-written TypeScript mirrors of the Rust IPC structs. Keep in sync with `src-tauri/src/commands` and the types they return. |
| `styles/` | `index.css` — the single global stylesheet (design tokens + component styles). |

## Conventions

- Talk to the backend with `invoke("<command>", { … })` from
  `@tauri-apps/api/core`; handle the rejection path (the command may return an
  `AstraError` string).
- Prefer routing user actions through Almanac (`astra:command` custom event)
  over adding parallel command paths, so the console stays the single log of
  what happened.
- New window app: add an `AppId` in `stores/windowStore.ts`, render it in
  `desktop/Desktop.tsx`, and (optionally) list it in the Start menu.
- Run `npm run check` before committing (Prettier + `tsc` + `vite build`).
