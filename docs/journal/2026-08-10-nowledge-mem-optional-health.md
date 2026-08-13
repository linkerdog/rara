# Nowledge Mem Optional Health

## Summary

Nowledge Mem session capture failures now remain internal health-check
degradation signals. RARA logs the degraded state and retries later, but no
longer publishes a runtime warning into the session transcript when the local
Nowledge Mem endpoint is missing or unavailable.

## Background

Not every developer, CI runner, or reviewer has Nowledge Mem installed. The
builtin integration is a context enhancement layer, not a hard dependency for
normal build, test, release, or agent execution.

## Scope

- Kept Nowledge Mem session capture best-effort.
- Changed failed capture and timeout paths from user-visible runtime warnings
  to internal degraded logs.
- Preserved retry behavior so a later healthy endpoint can accept the same
  unsent session messages.
- Documented the optional health-check contract in the builtin plugin spec.

## Validation

- `cargo test memory_lifecycle -- --nocapture`
- `cargo check -p rara --locked`
- `cargo fmt --all -- --check`
- `git diff --check`
