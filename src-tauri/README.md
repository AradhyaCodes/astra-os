# `src-tauri/` — Astra OS system core (Rust)

The authoritative system layer. It owns every stateful decision; the React
frontend in `../src` is a view over the contracts exposed here through Tauri
commands.

## Module map (`src/`)

| Module | Responsibility |
|---|---|
| `state.rs` | `SystemState` — the single object that ties the subsystems together and is the source of system truth. `transact()` gives a mutation atomicity + persistence. |
| `almanac/` | `lexer` → `parser` → `ast` → `engine`. The engine evaluates one command against `SystemState`; unrecognised lines fall through to the controlled host shell. |
| `filesystem/` | `model` (in-memory tree; files hold UTF-8 text *or* raw bytes), `operations` (create/read/write/move/copy/delete/search), `validation` (names, depth, total-size budget), `tree_parser` (`A>(B,C)`), `path` (normalisation). |
| `fs_provider/` | `router` decides whether a path is virtual or `HOST>…`; `host` is the mount table + safe, containment-checked host file access. |
| `scheduler/` | Round-robin / FCFS / priority strategies over a virtual multi-core CPU; per-core utilisation and averages. |
| `memory/` | Frame table, page tables, swap, FIFO/LRU replacement; fault/hit accounting. |
| `process/` | Process registry (simulated + host-backed) and `host_apps` — resolving and listing installable Windows applications. |
| `security/` | Argon2 profile login, failed-attempt lockout, per-subtree password locks (Astra-scoped only). |
| `persistence/` | `JsonPersistence` — flushed-temp + backup-swap snapshot writes, corruption quarantine, oversize-file sidelining. |
| `commands/` | The thin `#[tauri::command]` IPC boundary. No policy lives here. |
| `kernel/` | Static configuration constants and the `SystemConfig` surface. |
| `lib.rs` | Tauri app setup, state wiring, host-dir resolution, command registration. |

## Contracts

- Commands return `Result<T, AstraError>`; `AstraError` serialises to a plain
  string for the frontend.
- Virtual vs. host boundaries must stay visible in every response. Host
  destructive actions go to the Recycle Bin and require confirmation.
- Persistence rewrites the whole snapshot per mutation — respect the
  filesystem size budget in `filesystem::validation`.

## Checks

```bash
cargo test   --all-targets
cargo clippy --all-targets
cargo fmt -- --check
```
