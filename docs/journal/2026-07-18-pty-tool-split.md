# PTY Tool Module Split

## Summary

The PTY tool implementation is now split into real Rust submodules under
`src/tools/pty/`. The top-level `src/tools/pty.rs` file is a small facade with
module declarations and targeted re-exports.

## Background

`src/tools/pty.rs` was the remaining P0 file-size item in `docs/todo.md`. The
previous blocker was the inline test module shape, but the tests could be moved
into normal child modules once the PTY state, process, output, input, and tool
implementation boundaries were separated.

## Scope

- Moved PTY session state and lifecycle into `src/tools/pty/store.rs`.
- Moved tool structs, snapshots, session status, and input parsing into
  `src/tools/pty/types.rs`.
- Moved `Tool` implementations and `tool_spec` definitions into
  `src/tools/pty/tools.rs`.
- Moved process-kill helpers into `src/tools/pty/process.rs`.
- Moved PTY output tail reading into `src/tools/pty/output.rs`.
- Moved the existing PTY tests into `src/tools/pty/tests.rs` and
  `src/tools/pty/input_tests.rs`.

No behavior or public tool schema was intentionally changed.

## Validation

```bash
cargo fmt
cargo test --locked tools::pty -- --nocapture
cargo check --locked --workspace --all-targets
```

Focused PTY tests passed: 17 passed, 0 failed. The test build emitted the
existing macOS linker warning about a large `__eh_frame` section.

## Follow-Ups

- The next PTY cleanup can audit reader-thread log write errors and idle
  pruning semantics as behavior changes in separate focused patches.
