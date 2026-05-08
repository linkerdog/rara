# Windows CI Build Gate

## Context

The release workflow already builds Windows archives for
`x86_64-pc-windows-msvc` and `aarch64-pc-windows-msvc`, but pull-request CI only
validated Linux build, test, clippy, and formatting jobs.

That left Windows compile failures to surface late, during tag release builds.

## Change

The `build` workflow now includes a dedicated `build-windows` job:

- runner: `windows-latest`;
- toolchain: pinned `nightly-2026-05-02`, matching existing CI;
- dependency setup: `arduino/setup-protoc`;
- command: `cargo build --locked`.

This intentionally starts as a compile gate only. Windows tests can be added as
a later focused step after terminal and TUI behavior is audited on Windows.

## Validation

The PR should pass:

- Linux `build`;
- Windows `build-windows`;
- `fmt`;
- `clippy`;
- `test`.
