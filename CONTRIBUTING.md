# Contributing to Astra OS

Discuss substantial changes in an issue before implementation. Keep changes
focused and include regression tests for bugs. The project's license is still
pending a maintainer decision; resolve licensing before accepting external code
contributions or distributing derivative builds.

## Local checks (Windows)

Install Node.js 22.12+ in the 22.x line, stable Rust (MSVC), and Visual Studio C++
Build Tools. Use `npm ci` to install the committed dependency versions.

```powershell
npm run check
cargo fmt --manifest-path src-tauri/Cargo.toml -- --check
cargo test --locked --manifest-path src-tauri/Cargo.toml
cargo clippy --locked --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings
npm audit
cargo install cargo-audit --version 0.22.2 --locked
cargo audit --file src-tauri/Cargo.lock
```

Native development uses `npm run tauri dev`. A Vite-only browser cannot exercise
Tauri IPC. Test HOST operations only in disposable directories, without elevation.

## Change requirements

- Keep permissions, authentication and persistence authoritative in Rust.
- Keep Rust IPC output and TypeScript contracts consistent.
- Preserve existing profiles and legacy path compatibility, or provide a tested
  migration. Never silently reset unsupported future data formats.
- Never overwrite an existing destination during a copy. Report partial copies;
  leave the source intact on errors. Host writes are not virtual operations.
- Add tests for cancellation, failure and denied permissions, not only success.
- Commit lockfiles when dependencies change. Do not commit state snapshots,
  signing keys, credentials, installers or generated build directories.
- Describe what was and was not tested. Do not claim security isolation, playable
  games or production support where the implementation does not provide it.

Use `SECURITY.md` for vulnerabilities rather than public exploit reports.
