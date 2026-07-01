# Code Health Review — 2025

## Problem

A comprehensive review of the `src/` tree revealed several structural issues
that violate the project's own architecture constraints. The most pressing
problems are:

1. **Oversized modules** — five source files exceed the 2000-line guideline
   established in AGENTS.md §3, with `src/tui/state/mod.rs` exceeding 2000
   lines.
2. **Dead code accumulation** — 71 `#[allow(dead_code)]` annotations spread
   across production files, including an entire module (`src/runtime_control.rs`)
   that is nearly all scaffolding.
3. **Agent struct encapsulation** — the `Agent` struct exposes nearly all
   fields as `pub`, allowing external code to bypass invariants.
4. **TUI state monolith** — `src/tui/state/mod.rs` mixes types, presets,
   persistence, and business logic in a single file.

This spec defines the target state, contracts, and rollout plan for fixing
these issues.

## Scope

- Split oversized source files that exceed 1000 lines.
- Remove dead code from production compilation units.
- Narrow `Agent` field visibility to `pub(crate)` or private where safe.
- Add AGENTS.md enforcement rules to prevent regression.
- Track remaining work in `docs/todo.md`.

## Non-Goals

- No behavior changes to the agent loop, TUI rendering, or public API.
- No new features or abstractions beyond what splitting requires.
- No crate-level reorganization (crate boundaries are unchanged).

## Architecture

### Target File Sizes

| File | Current Lines | Target | Strategy |
|---|---|---|---|
| `src/tui/state/mod.rs` | 2000+ | ≤300 | Split out types, presets, persistence, provider-status into submodules |
| `src/agent.rs` | 1287 | ≤500 | Extract tool-execution, plan-handling, history-management into `agent/` submodules |
| `src/agent/compact/main.rs` | ~1250 | ≤500 | Split by compaction phase: microcompact, full-compact, strategy |
| `src/tui/render.rs` | 990 | ≤500 | Move remaining top-level functions into existing submodules |
| `src/context/assembler.rs` | 916 | ≤500 | Extract budget calculation, message assembly, and system-prompt building |

### Dead Code Removal Policy

Delete dead code from the source tree. Do not move scaffolding into separate
files to preserve it. If a type, function, or constant is not reachable from
any `pub` entry point or test, remove it.

**Exception**: `src/tui/theme.rs` color palette constants may be kept with a
single `#![allow(dead_code)]` on the module, documented with a comment that
they are reserved for planned rendering features. Remove unused individual
constants.

**Priority files for dead-code removal**:

- `src/runtime_control.rs` — remove all unused types and enums
- `src/hook_registry.rs` — remove unused `all_hooks` method
- `src/acp_consumer.rs` — remove unused types and methods
- `src/mcp_status.rs` — remove unused type
- `src/tui/custom_terminal.rs` — remove unused method

### Agent Encapsulation

Change `Agent` struct field visibility:

- `pub` → `pub(crate)` for fields accessed only within `crate::` and `crate::agent::`
- `pub` → private for fields accessed only within `Agent` impl blocks
- Add accessor methods only where external visibility is truly needed

**This must be done after file splitting** to avoid massive churn on a
monolithic file.

### AGENTS.md Enforcement Rules

Add the following to AGENTS.md §3.1:

- Dead code is not permitted in production source files. Use `#[cfg(test)]`
  for test-only helpers; remove everything else.
- File-size violations (source files exceeding 1000 lines under `src/` or
  `crates/`) detected in review must be fixed before merge, not deferred.
- Adding `#[allow(dead_code)]` requires a comment explaining why the code is
  intentionally unused and when it will be activated.
- `mod.rs` files shall be facades: module declarations and re-exports only.
  They must not contain business logic. Pure import/re-export size is not a
  concern.

## Contracts

### File Size

- No source file under `src/` or `crates/` shall exceed 1000 lines.
- `mod.rs` files shall be facades only (module declarations + re-exports),
  no business logic. Pure import size is not a concern.
- These limits are enforced by review, not by tooling, until a CI gate is
  added.

### Dead Code

- `cargo check` shall produce zero dead-code warnings for `src/` and `crates/`.
- Exceptions are documented in the affected module with a comment linking to
  the planned activation milestone in `docs/todo.md`.

### Agent Encapsulation

- `Agent` struct fields are `pub(crate)` or private.
- External mutation of `Agent` state goes through named methods, not direct
  field assignment.

## Validation Matrix

| Check | How |
|---|---|
| File sizes ≤1000 lines | `wc -l` on each `src/**/*.rs` |
| No new dead-code warnings | `cargo check` for `src/` |
| Agent fields private/pub(crate) | Code review of `src/agent.rs` |
| TUI snapshot tests pass | `cargo test` — tui snapshot suite |
| Agent loop unchanged | `cargo test` — agent tests |
| Formatting unchanged | `cargo fmt --check` |
| Clippy clean on touched files | `cargo clippy` scoped to changed modules |

## Operational Notes

- Splitting files preserves git history best with `git mv`-style refactoring
  where practical, but exact history preservation is not a requirement —
  behavior preservation is.
- Each split should be its own commit to keep review manageable.
- Dead-code removal commits should be separate from file-split commits.
- Agent encapsulation should be the last step after file splitting to avoid
  rebase conflicts.

## Open Risks

- Some `pub` fields may be accessed by external crates or integration code
  not visible in the current workspace. Run `cargo check --all-targets` after
  each visibility change.
- Splitting `src/agent/compact/main.rs` may expose implicit dependencies on
  `Agent` private fields that are currently accessible because the impl is in
  the same module.

## Source Journals

