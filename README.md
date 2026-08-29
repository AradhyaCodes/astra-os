# Aaru-OS

A Windows-inspired **operating-system simulator** that runs as a real desktop
application. The graphical desktop is a thin view layer; a Rust system core
owns a persistent virtual filesystem, a deterministic CPU scheduler, a
simulated memory/paging model, a process registry, a security model, and the
**Almanac** command language. Real Windows folders and applications are reached
only through explicit, clearly-labelled bridges.

> Built with **Tauri 2 · React 19 · TypeScript · Rust**.

---

## Table of contents

- [Highlights](#highlights)
- [Prerequisites](#prerequisites)
- [Getting started](#getting-started)
- [Building a release](#building-a-release)
- [The Almanac command language](#the-almanac-command-language)
- [HOST bridge — using real files and apps](#host-bridge--using-real-files-and-apps)
- [How state is stored](#how-state-is-stored)
- [Project layout](#project-layout)
- [Testing & checks](#testing--checks)
- [What Aaru-OS is *not*](#what-aaru-os-is-not)

---

## Highlights

| Area | What it does |
|---|---|
| **Virtual filesystem** | In-memory tree with a canonical `ROOT>Documents>Projects` path form. Case-sensitive names, depth-limited, text **and** binary payloads, per-resource permissions and password locks. |
| **Almanac shell** | A single command language for navigation, file operations, process/scheduler/memory inspection, host mounts, and lifecycle. Type `almanac` in the console for the full reference. |
| **CPU scheduler** | Round-robin / FCFS / priority, switchable at runtime, with per-core utilisation, ready-queue, context-switch counts and wait/turnaround/response averages. |
| **Memory model** | Simulated RAM, frames, swap and page tables with FIFO or LRU replacement; page-fault / hit counters. Independent of real Windows memory. |
| **Process registry** | Simulated and host-backed processes in one table, with terminate / suspend / resume. |
| **Security** | Local profile password (Argon2 hashes only), lockout after repeated failures, and password-locked directory subtrees — inside Aaru only. |
| **HOST bridge** | Mount a Windows folder as `HOST>alias` for live in-place read/write; launch installed Windows apps; copy/transfer between `HOST>` and the virtual tree. |
| **Windows Apps panel** | Detects which known desktop / Microsoft Store apps are installed and launches them (`claude`, `chatgpt`, `vsc`, `antigravity`, `chrome`, …). |
| **Lifecycle** | Login, logout, hibernate (runtime image), restart, and "kill LapSession" — all from the Start menu or Almanac. |

---

## Prerequisites

- **Node.js** `^20.19` (20.x line) or `>= 22.12`
- **Rust**, stable toolchain (MSVC on Windows)
- **Windows:** Visual Studio Build Tools with **Desktop development with C++**
  (the linker `link.exe` is required by the Rust compiler)

If `cargo` is not found after installing Rust, open a new terminal so the
updated `PATH` takes effect.

---

## Getting started

```bash
npm install
npm run tauri dev
```

The first run compiles the Rust crate (a few minutes) and opens the desktop
window; subsequent runs hot-reload both sides.

`npm run dev` starts **only** the Vite frontend. Tauri IPC is unavailable in a
plain browser tab, so system calls will show a recoverable connection error
there — use `npm run tauri dev` for anything real.

On first launch Aaru-OS asks you to set a **local profile password**. It is
stored only as a salted Argon2 hash.

---

## Building a release

```bash
npm run tauri build
```

Installers are written to `src-tauri/target/release/bundle/` (NSIS `-setup.exe`
and MSI on Windows). The first build downloads the bundler toolchains.

---

## The Almanac command language

Open the **Almanac** console (Start menu, the taskbar terminal icon, or
`Ctrl` + `` ` ``) and type `almanac` for the built-in reference. Paths use `>`
as the separator; `AARU` and `ROOT` both name the virtual root.

| Command | Purpose |
|---|---|
| `open <path> [in <App>]` | Enter a directory — or open a file / `HOST>` folder in an app |
| `back` · `root` · `scan` | Navigate / list the current directory |
| `gen <path>` · `mgen <expr>` | Create a directory / a tree, e.g. `Projects>(A,B,C)` |
| `write <file> [in <App>]` | Create a file, optionally open it |
| `rewrite <file> [in <App>]` | Edit an existing file |
| `destroy <path>` | Delete a subtree (asks first) |
| `rename <path>>newName` | Rename a resource |
| `transfer <from> <to>` · `copy <from> <to>` | Move / copy — **HOST ↔ AARU allowed** |
| `lookout <term>` | Search accessible resources |
| `inspect <path>` | Show resource metadata |
| `lock <path>` · `unlock <path>` | Password-lock a directory (Aaru-only) |
| `mount [path]` · `unmount <alias>` · `mounts` | Manage HOST folders |
| `run <App> [args]` | Launch a built-in app or a real Windows app |
| `reveal HOST><path>` | Open a host file with its default Windows app |
| `process` · `terminate\|suspend\|resume <pid>` | Inspect / manage the process table |
| `scheduler [change <algo>] [tick <n>]` | Inspect or drive the virtual CPU |
| `memory [policy <FIFO\|LRU>]` | Inspect the RAM / swap / paging model |
| `logout` · `kill lapsession` · `hibernate` · `restart` | Lifecycle |

Quick launch words (also usable as `in <App>`): `claude`, `chatgpt`, `google`,
`vsc`, `antigravity`, `brave`, `chrome`. Anything the parser doesn't recognise
is handed to the controlled host shell (`git`, `npm`, `python`, …).

Quote any path segment that contains spaces:
`almanac open "HOST>Desktop>My Project>app.js" in vsc`.

---

## HOST bridge — using real files and apps

Mount a real folder, then work on it in place. Nothing is copied into the
virtual filesystem — only the mount's alias and path are persisted.

```text
almanac mount "C:\Users\you\Desktop\my-project"   # → HOST>my-project
almanac open  HOST>my-project>src>index.ts in vsc # edit the real file
almanac open  HOST>my-project in vsc              # open the folder as a project
almanac reveal HOST>my-project>report.pdf         # Windows default app
```

`copy` / `transfer` **can** cross the boundary (`copy HOST>… Documents` or the
reverse). Cross-boundary copy is bounded: **4 MiB per file**, **64 MiB total**
in the virtual filesystem; oversize or unreadable entries are skipped and
reported. Prefer working in place under `HOST>` for anything large.

---

## How state is stored

Durable state — the virtual filesystem (including binary payloads, base64), its
metadata, permissions, lock hashes, settings, command history and host-mount
records — is saved to `state.json` in Tauri's app-data directory
(`%APPDATA%\com.aaru.os\` on Windows). Writes go through a flushed temp file
and a backup swap so an interrupted write can recover.

Login sessions, failed-attempt counters and authenticated lock boundaries are
**process-local** and deliberately do not survive a restart.

The whole document is rewritten on every mutation, so the virtual filesystem is
capped at 64 MiB of file content. A `state.json` that somehow grows past 96 MiB
is set aside on boot (`state.oversized-<timestamp>.json`) and a fresh one is
started.

---

## Project layout

```text
src/                     React desktop (view layer only)
  almanac/               Almanac console UI
  applications/          Built-in apps (Settings, Task Manager, Windows Apps, …)
  desktop/               Desktop, taskbar, window chrome
  stores/                Zustand window store
  types/                 TypeScript mirrors of the Rust IPC structs
src-tauri/               Rust system core
  src/
    almanac/             Lexer, parser, AST, command engine
    filesystem/          Virtual filesystem model, operations, validation
    fs_provider/         Path router + HOST bridge
    scheduler/           CPU scheduler + strategies
    memory/              Frames, page tables, swap, replacement policy
    process/             Process registry + host-app resolution
    security/            Auth, lockout, resource locks
    persistence/         Durable JSON snapshot store
    commands/            Tauri IPC boundary
    state.rs             SystemState — the single source of system truth
```

See [`src/README.md`](src/README.md) and [`src-tauri/README.md`](src-tauri/README.md)
for module-level notes, and [`PRODUCT.md`](PRODUCT.md) / [`DESIGN.md`](DESIGN.md)
for the product and visual-design briefs.

---

## Testing & checks

```bash
npm run check                                  # prettier + tsc + vite build
cargo test  --manifest-path src-tauri/Cargo.toml
cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets
cargo fmt   --manifest-path src-tauri/Cargo.toml -- --check
```

---

## What Aaru-OS is *not*

- It does **not** encrypt, hide, or change permissions on real Windows files.
  An Aaru "lock" only gates access inside Aaru-OS.
- Mounted host resources are real. Destructive host actions (`destroy`,
  cross-boundary `transfer`) move files to the Windows Recycle Bin and prompt
  first.
- The scheduler, memory and process simulation are teaching models. Tracked
  host processes are *observed* — Windows schedules them, not Aaru.

---

## License

No license has been chosen yet. Add a `LICENSE` file before making the
repository public if you intend others to reuse the code.
