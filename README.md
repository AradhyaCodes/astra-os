# Astra OS

<img src="public/astra-logo.png" alt="Astra OS logo" width="96" />

A Windows-inspired **operating-system simulator** that runs as a real desktop
application. The graphical desktop is a thin view layer; a Rust system core
owns a persistent virtual filesystem, a deterministic CPU scheduler, a
simulated memory/paging model, a process registry, a security model, and the
**Almanac** command language. Real Windows folders and applications are reached
only through explicit, clearly-labelled bridges.

> Built with **Tauri 2 · React 19 · TypeScript · Rust**.

The app uses a single-color violet **A** mark across the favicon, in-app UI,
Windows shortcuts, and packaged application icons.

Status: **pre-release educational software**. HOST commands execute real Windows
programs and can modify real files; Astra is not a sandbox. Use a standard user
account and disposable folders for experiments. Stable release gates are tracked
in [the release checklist](docs/RELEASE_CHECKLIST.md).

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
- [What Astra OS is *not*](#what-astra-os-is-not)

---

## Highlights

| Area | What it does |
|---|---|
| **Virtual filesystem** | In-memory tree with a canonical `ROOT>Documents>Projects` path form. Case-sensitive names, depth-limited, text **and** binary payloads, per-resource permissions and password locks. |
| **Almanac shell** | A single command language for navigation, file operations, process/scheduler/memory inspection, host mounts, and lifecycle. Type `almanac` in the console for the full reference. |
| **CPU scheduler** | Round-robin / FCFS / priority, switchable at runtime, with per-core utilisation, ready-queue, context-switch counts and wait/turnaround/response averages. |
| **Memory model** | Simulated RAM, frames, swap and page tables with FIFO or LRU replacement; page-fault / hit counters. Independent of real Windows memory. |
| **Process registry** | Simulated and host-backed processes share one table. Astra can terminate processes it launched; suspend / resume applies to simulated processes only. |
| **Security** | Local profile password (Argon2 hashes only), lockout after repeated failures, and password-locked directory subtrees — inside Astra only. |
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

On first launch Astra OS asks you to set a **local profile password**. It is
stored only as a salted Argon2 hash.

---

## Building a release

```bash
npm run tauri build
```

Installers are written to `src-tauri/target/release/bundle/` (NSIS `-setup.exe`
and MSI on Windows). The first build downloads the bundler toolchains.

To build only the Windows MSI:

```bash
npm run tauri -- build --bundles msi
```

The MSI is written to `src-tauri/target/release/bundle/msi/`. After changing
Windows icon files, run
`cargo clean --manifest-path src-tauri/Cargo.toml --release` before rebuilding
so Cargo regenerates the executable's embedded icon resource.

Local and CI validation builds are unsigned unless signing is configured by the
maintainer. Building an MSI does not make it a trusted production release.

---

## The Almanac command language

Open the **Almanac** console (Start menu, the taskbar terminal icon, or
`Ctrl` + `` ` ``) and type `almanac` for the built-in reference. Paths use `>`
as the separator; `ASTRA` and `ROOT` both name the virtual root.
The previous `AARU>` and `AARU::` path forms remain accepted for compatibility.

| Command | Purpose |
|---|---|
| `open <path> [in <App>]` | Enter a directory — or open a file / `HOST>` folder in an app |
| `back` · `root` · `scan` | Navigate / list the current directory |
| `gen <path>` · `mgen <expr>` | Create a directory / a tree, e.g. `Projects>(A,B,C)` |
| `write <file> [in <App>]` | Create a file, optionally open it |
| `rewrite <file> [in <App>]` | Edit an existing file |
| `destroy <path>` | Delete a subtree (asks first) |
| `rename <path>>newName` | Rename a resource |
| `transfer <from> <to>` · `copy <from> <to>` | Move / copy — **HOST ↔ ASTRA allowed** |
| `lookout <term>` | Search accessible resources |
| `inspect <path>` | Show resource metadata |
| `lock <path>` · `unlock <path>` | Password-lock a directory (Astra-only) |
| `mount [path]` · `unmount <alias>` · `mounts` | Manage HOST folders |
| `run <App> [args]` | Launch a built-in app or a real Windows app |
| `reveal HOST><path>` | Open a host file with its default Windows app |
| `process` · `terminate\|suspend\|resume <pid>` | Inspect / manage the process table |
| `scheduler [change <algo>] [tick <n>]` | Inspect or drive the virtual CPU |
| `memory [policy <FIFO\|LRU>]` | Inspect the RAM / swap / paging model |
| `logout` · `kill lapsession` · `hibernate` · `restart` | Lifecycle |

Bare quick-launch words are `claude`, `chatgpt`, `google`, `vsc`, `antigravity`,
`brave` and `chrome`. Desktop apps that accept file paths can also be used with
`in <App>`. Input that is neither an `almanac ...` command nor a quick-launch
word is sent to the controlled host shell (`git`, `npm`, `python`, …).

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
metadata, permissions, lock hashes, command history and host-mount records — is
saved to `state.json` in Tauri's app-data directory
(`%APPDATA%\com.astra.os\` on Windows). Writes go through a flushed temp file
and a backup swap so an interrupted write can recover.

On the first Astra OS launch, an existing `%APPDATA%\com.aaru.os\state.json`
profile is validated and saved atomically into the new app-data directory if
Astra has neither a primary snapshot nor a backup. The original is left intact.
Only one Astra app instance is allowed to run for this application identity.

Login sessions, failed-attempt counters and authenticated lock boundaries are
**process-local** and deliberately do not survive a restart.

The whole document is rewritten on every mutation, so the virtual filesystem is
capped at 64 MiB of file content. A `state.json` that somehow grows past 96 MiB
is set aside on boot (`state.json.oversized-<timestamp>.json`). The same read limit
applies to the backup. One previous committed snapshot is retained as
`state.json.bak`; if the primary is corrupt or oversized, recovery tries that
backup before starting fresh. Unsupported schema versions stop startup without
resetting the profile. Keep separate backups before upgrading.
Loaded virtual filesystems are checked for invalid links, cycles, inconsistent
IDs/parents, excessive depth and incorrect payload sizes before runtime use.

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

The Release checks workflow runs the frontend checks, Rust tests and Clippy,
dependency audits, a redacted Git-history secret scan, and an unsigned Windows
MSI build. See [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), and
[CHANGELOG.md](CHANGELOG.md). A workflow definition is not evidence of a passing
remote run; check the Actions result for the release commit.

---

## What Astra OS is *not*

- It does **not** encrypt, hide, or change permissions on real Windows files.
  An Astra "lock" only gates access inside Astra OS.
- Mounted host resources are real. `destroy` previews the affected items, asks
  for confirmation, and moves them to the Windows Recycle Bin.
- Almanac asks for confirmation before a `transfer` involving HOST resources.
  Cross-boundary transfer copies first and removes the source only when no
  entries were skipped. A `HOST>` source is moved to the Recycle Bin.
- New-file copies refuse existing destinations. Recursive host copies reject
  symbolic links, junctions and reparse points. A failed multi-file copy can
  leave a partial destination; the operation is not a cross-filesystem transaction.
- Some registered apps/games use a process-information placeholder rather than
  a complete application or playable game.
- The scheduler, memory and process simulation are teaching models. Tracked
  host processes are *observed* — Windows schedules them, not Astra.

---

## License

No license has been chosen yet. Add a `LICENSE` file before making the
repository public if you intend others to reuse the code.
