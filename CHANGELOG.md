# Changelog

## Unreleased

- Keep one previous committed profile snapshot for recovery.
- Bound reads of both primary and backup profiles; quarantine malformed or
  oversized files independently, and refuse unsupported schemas without reset.
- Validate persisted filesystem links, parent relationships, IDs, depth and
  payload sizes before initializing the runtime.
- Migrate legacy profiles through validated, flushed writes without replacing
  an existing primary or backup.
- Register single-instance handling to prevent two Astra runtimes writing the
  same profile.
- Ask for confirmation before Almanac transfers involving HOST resources.
- Reject copy destination collisions, mount-root moves and recursive copies
  through symlinks/junctions/reparse points. Propagate directory-read failures
  so a partial copy cannot be mistaken for success before source removal.
- Add Windows build/test/audit CI, a redacted history secret scan, dependency
  update configuration, contribution guidance and explicit release gates.

This is a pre-release hardening change, not a declaration of production readiness.
