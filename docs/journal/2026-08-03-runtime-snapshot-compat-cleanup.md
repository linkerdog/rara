# Runtime Snapshot Compatibility Cleanup

## What changed

Removed the TUI-owned extension discovery path and the
`TuiApp::sync_snapshot(&Agent)` entry point. Restore, command, and completion
paths now apply a typed `RuntimeExtensionSnapshot` produced by runtime code.

## Why

Extension discovery is runtime work. Keeping it in the TUI snapshot reducer
allowed presentation code to recreate hook and agent registries and made the
runtime snapshot contract dependent on an `Agent` implementation detail.

## Remaining boundary

The in-process task bridge still needs registry handles to execute control
requests. Those handles remain in the transitional TUI state until task
construction is fully passed through the runtime processor. They are no
longer used to discover or assemble TUI extension status.

## Verification

- `cargo fmt --all`
- `cargo check --all-targets`
- `cargo test --bin rara tui::state::tests::sync_snapshot -- --nocapture`
