# Release gates

Status: pre-release hardening in progress. Passing checks is not a production
certification. The supported release target is Windows x64; Linux/macOS are not
validated release targets.

## Required before a stable public release

- [ ] Maintainer chooses a license; add LICENSE and matching npm/Cargo metadata.
- [ ] Review rights and notices for redistributed dependencies and artwork.
- [ ] Run Release checks successfully on GitHub from the exact release commit.
- [ ] Enable private vulnerability reporting and protect the release branch.
- [ ] Review all RustSec informational advisories. No blanket ignores.
- [ ] Test install, upgrade, uninstall and reinstall on a clean Windows VM with
      a standard (non-admin) account. Check WebView2 installation and offline use.
- [ ] Verify password setup/login/logout, failed-attempt lockout, nested locks,
      hibernate/resume, and restart in the installed build.
- [ ] Verify a second launch focuses the existing window rather than starting
      another writer. Test an interrupted save and recovery from the backup.
- [ ] Verify migration with a COPY of an older profile, including a hibernate
      snapshot. Test unsupported future-version refusal without file changes.
- [ ] Exercise HOST writes, collisions, cancelled transfers, partial copies,
      locked resources and junctions using disposable directories.
- [ ] Test external editor changes, disk-full/read-only errors and process
      cleanup; do not run tests against personal files.
- [ ] Complete keyboard navigation, scaling and accessibility checks in the
      native app. Browser-only checks do not validate IPC or Windows integration.
- [ ] Sign the installer with the maintainer's Windows code-signing identity and
      verify the signature. Never commit the private signing key.
- [ ] Publish a versioned artifact with its SHA-256 checksum and honest release
      notes only after these gates pass. Keep 0.1.x labeled pre-release meanwhile.

## Current dependency review

The initial hardening audit on 2026-08-30 reported no RustSec vulnerability
entries, but did report unmaintained GTK3/proc-macro-error/UNIC dependencies and
the GLib advisory RUSTSEC-2024-0429 (unsoundness). The Windows x64 dependency tree
does not include GLib; UNIC remains transitive through Tauri's urlpattern stack.
These are tracked upstream concerns, not proof of a clean supply chain. Re-run
the audit against the final lockfile and current advisory database for a release.

## Local validation snapshot (2026-08-30)

The hardening working tree passed the following checks on the development
machine. These results do not replace the installed-build checks above.

- 184 Rust tests passed, including recovery, transfer cancellation, copy
  collision and Windows junction regressions.
- Frontend checks/build, release metadata checks, Rust formatting and strict
  Clippy checks passed.
- npm audit reported zero vulnerabilities. RustSec reported zero vulnerability
  entries, with 16 unmaintained-package warnings and one unsoundness warning
  retained for review as described above.
- Redacted Gitleaks scans of committed history, the tracked diff, and new
  workflow/script/release-doc directories reported no leaks.
- Actionlint accepted the workflow locally; ShellCheck was not available.
  GitHub Actions has not run this new workflow yet.
- The locked Windows release build produced
  `src-tauri/target/release/bundle/msi/Astra OS_0.1.0_x64_en-US.msi`.
  Windows reports its signature status as `NotSigned`.

SHA-256 for that specific local MSI (rebuilds may differ):

```text
BCDE7C9BE3A3ED72BF0D3F3BB803DCF7B985FF6368FBEDE82DF09126AF39A0D8
```

## Known limitations

- Registered games that use AppShell show process/workload information; they
  are not complete playable games.
- Host programs are not sandboxed. Do not use Astra to execute untrusted code.
- Host edits and multi-file transfers are not transactions across Windows and
  the virtual filesystem. A failed copy can leave a partial destination; inspect
  it before retrying. Concurrent external changes are not comprehensively locked.
- A local profile password is an in-app access gate, not disk encryption.
- Local tests do not establish installer trust, independent security review or
  production readiness. Unsigned validation MSIs are for testing only.
