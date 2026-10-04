# Workspace Resolution

## Summary

Complete the explicit Cargo resolver and default-member settings requested by
#871 Phase 0. The root workspace uses resolver `3` and selects only the root
application by default. Existing workspace lint inheritance and package-specific
native/browser commands remain unchanged.

## Background And Decisions

The workspace already uses edition 2024 and shared Clippy denies, but explicitly
retained resolver 2 and left default package selection implicit. Cargo metadata
confirmed that commands from the root selected only the root application.

The inspected Codex workspace at `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
centralizes member and lint policy but still uses resolver 2. Retain that policy
ownership pattern while following this issue's resolver-3 requirement. The
inspected Claude Code snapshot at `4b9d30f7953273e567a18eb819f4eddd45fcc877`
contains TypeScript sources without a build manifest and provides no equivalent
Cargo policy to adapt.

According to the [Cargo resolver documentation](https://doc.rust-lang.org/cargo/reference/resolver.html#resolver-versions),
resolver 3 changes Rust-version compatibility preference from `allow` to
`fallback`. This change establishes neither a new project MSRV nor a dependency
upgrade. The pinned toolchain already supports the resolver.

Explicit `default-members = ["."]` preserves the existing root build/test scope.
Selecting all members by default would change everyday command scope and cost.
Shared crates remain selected through `-p`, and `--workspace` selects the entire
workspace. The [workspace specification](../features/crate-split.md#workspace-build-policy)
records that downstream workspaces retain their own resolution policy.

## Validation

Before and after the manifest change, `cargo metadata --locked --offline
--format-version 1 --filter-platform <target>` produced identical package
records, feature selections, dependency edges, workspace membership, and default
members. The whole-workspace metadata contains 1,002 resolved packages for
`x86_64-unknown-linux-gnu` and 946 for `wasm32-unknown-unknown`. These counts are
metadata comparisons, not browser portability claims for native members.
`Cargo.lock` is byte-for-byte unchanged. `cargo tree --depth 0 --prefix none`
selects one default member and 23 members with `--workspace`.

- `cargo clippy --locked --offline --workspace --all-targets -- -D warnings`:
  passed.
- `cargo test --locked --offline -p rara-core -p rara-agent`: 9 core and 29 agent
  tests passed, none ignored.
- `cargo check --locked --offline --target wasm32-unknown-unknown -p rara-core
  -p rara-agent`: passed.
- `cargo build --locked --offline`: the default root application build passed.
- `cargo fmt --all`: passed.

Local `bazel build //...` stopped before compilation because external repository
files were missing: first `rules_rust`, then `platforms` and `rules_shell` after
targeted restoration. The dependency generator accepted resolver 3 and changed
only the recorded `Cargo.toml` input hash in `MODULE.bazel.lock`; generated
packages and build definitions remain identical. Full default Bazel build/test
validation is left to the remote CI runner rather than changing local Bazel
configuration. No production Rust behavior or test implementation changed;
existing executable checks validate integration.

## Follow-Ups

Provider and context extraction, browser HTTP/SSE transport and session
scheduling, and incremental cleanup of legacy lint allowances remain #871 work.
Resolver selection does not complete those contracts. Independent downstream
Git validation, exact-head CI, review, and merge remain delivery gates.
