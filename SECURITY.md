# Security policy

Astra OS 0.1.x is a pre-release, Windows-hosted educational simulator. It has
not undergone an independent security assessment. No stable production-support
or response-time guarantee is offered yet.

## Trust boundary

Virtual processes, scheduling and memory are teaching models. HOST operations
are real: commands run with the permissions of the current Windows account and
can launch programs, write files and use the network. Astra is **not a sandbox**.
Do not run it as administrator or paste untrusted commands. Its local password
does not encrypt stored files or protect against another process running as you.

Command history, file content, mount paths and password hashes are stored in the
app-data directory. Do not share a profile snapshot or put tokens/passwords in
shell arguments. Recursive copies refuse symlinks/junctions/reparse points, but
the host bridge is not a security boundary against a hostile local process
racing filesystem changes. Close external editors before moving their files.

## Reporting a vulnerability

Use GitHub private vulnerability reporting on the repository's Security tab
when available. If it is not enabled, open an issue requesting a private contact
channel **without** exploit details, personal paths, credentials or profile data.
The maintainer must enable private reporting before a stable release.

Include the commit/version, Windows version, affected operation, expected and
actual behavior, and a minimal reproduction using disposable test files. Never
test destructive operations against someone else's files.

## Dependency checks

Run `npm audit` and `cargo audit --file src-tauri/Cargo.lock`. CI fails on reported
vulnerabilities; informational advisories remain visible and need review.
Do not add blanket advisory ignores or run automatic breaking upgrades just to
obtain a green check. See `docs/RELEASE_CHECKLIST.md` for the release gates.
