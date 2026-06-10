# RARA Project Charter

This document records RARA goals, scope, architecture constraints, and documentation rules.
It is the baseline index for future implementation and evolution.

---

## Index

### Project
- [1. Project Goals](#1-project-goals)
- [2. Scope](#2-scope)
- [3. Architecture Constraints](#3-architecture-constraints)
- [4. Current Key Decisions](#4-current-key-decisions)
- [5. Documentation Rules](#5-documentation-rules)
- [6. Near-Term Focus](#6-near-term-focus)
- [7. Repository Skills](#7-repository-skills)
- [8. Commit Rules](#8-commit-rules)

### Docs
- [docs/features/](docs/features/README.md) — engineering specs and contracts (28 files)
- [docs/journal/](docs/journal/) — dated implementation notes and checkpoints
- [docs/journal/2026-05-07-file-split-lessons.md](docs/journal/2026-05-07-file-split-lessons.md) — file splitting patterns, pitfalls, and workflow
- [docs/todo.md](docs/todo.md) — active follow-up work

---

## 1. Project Goals

RARA is a local-first coding agent runtime with:

- a terminal chat and TUI surface;
- pluggable LLM backends;
- an agent loop that can call tools and continue after tool results;
- durable local memory and workspace context;
- room for both hosted providers and local model execution.

The current product direction is to make local inference a first-class path instead of a fallback.

## 2. Scope

- Interactive TUI chat flow.
- Tool-calling agent loop.
- Local workspace and project memory.
- Hosted-provider integration where useful.
- Local model execution through Candle-backed runtimes.

## 3. Architecture Constraints

- Backend/runtime language: Rust.
- The primary execution surface is a local CLI/TUI binary.
- The agent loop should continue to depend on a stable backend trait instead of model-specific code paths.
- Local models should plug into the same `LlmBackend` contract used by hosted providers.
- TUI interaction should converge toward one unified prompt surface instead of growing separate setup-only flows for common actions.
- Prefer smaller modules over long files; as a rule of thumb, avoid letting a single source file grow beyond roughly 800 lines unless there is a strong reason not to split it.
- If an implementation would push a source file toward or past that limit, proactively split the file instead of continuing to accumulate new logic in place.
- Non-trivial behavior changes should add or update focused tests when practical.
- Before implementing any non-trivial behavior change, first inspect the relevant Codex and Claude Code implementations, extract the interaction or runtime pattern that applies, write a short plan for how RARA should mirror or adapt it, and only then start implementation.

## 3.1 Rust Engineering Rules

- Always run `cargo fmt` to maintain consistent code style.
- New or modified Rust code must not introduce new compiler or Clippy warnings.
  The existing codebase may still contain legacy warnings; keep changes
  warning-clean within the touched scope, and prefer fixing nearby warnings
  only when they are directly caused by or blocking the current change.
- Avoid ambiguous positional booleans, numeric literals, or `Option` arguments in new APIs when they make call sites hard to read. Prefer enums, newtypes, named methods, or small parameter structs.
- Prefer exhaustive `match` statements over wildcard arms when the variants are part of a meaningful state machine or protocol contract.
- Newly introduced traits should include concise doc comments that explain the trait role and what implementors must preserve.
- Keep modules private by default and expose public APIs intentionally through explicit module exports.
- Do not add helper functions that are only used once unless they name a non-obvious invariant or isolate a testable boundary.
- When adding a new concept, first check whether it belongs in an existing narrow crate/module or whether a small new module avoids growing a high-touch orchestration file.
- Avoid `unsafe` unless it is strictly necessary. When `unsafe` is required,
  isolate the smallest possible boundary and add a concise `// SAFETY:`
  comment explaining the invariant that makes the block sound.

- **Errors must surface.** Do not silently ignore errors. Use `log::warn!` for
  non-fatal errors so they appear in the TUI alongside the conversation. A silent `let _ = fallible_call();` or `.ok()` discard is
  only acceptable when the error carries zero diagnostic value AND the
  caller has no recovery path. When in doubt, log it.

- Dead code is not permitted in production source files. Use `#[cfg(test)]`
  for test-only helpers; remove all unreachable types, functions, and constants.
- `#![allow(dead_code)]` at module level is not permitted — it silently
  hides real dead code.  Every intentionally-unused item must carry its own
  `#[allow(dead_code)]` with a comment explaining why and when it activates.
- Adding `#[allow(dead_code)]` to an individual item requires a comment
  explaining why the item is intentionally unused and when it will be activated.
- File-size violations detected in review (source files exceeding 800 lines
  under `src/` or `crates/`) must be fixed before merge, not deferred to a
  follow-up task.
- `mod.rs` files shall be facades: module declarations and re-exports only.
  They must not contain business logic. Pure import/re-export size is not a
  concern, but any function body, type definition, or logic belongs in a
  submodule.

## 3.2 TUI Engineering Rules

- Keep TUI state, display data, and rendering separate. State modules should not build Ratatui `Line`, `Span`, color, or layout objects.
- Prefer semantic theme/style helpers over hardcoded colors. Avoid hardcoded white; use default foreground or theme constants unless a specific semantic color is required.
- Use shared wrapping and display-sanitization helpers instead of hand-rolled wrapping, ANSI stripping, or width accounting in individual renderers.
- User-visible TUI behavior changes should add or update focused render tests or snapshots when layout, ordering, labels, grouping, or card boundaries are the contract.
- Review generated snapshot changes before accepting them. Do not update snapshots just because the test output changed.

## 3.3 Runtime Prompt And Tool Guidance

- Rules that shape agent behavior across all tasks belong in the default prompt only when they materially affect task completion, correctness, or safety.
- Tool-selection rules belong in the relevant tool descriptions when the model needs the rule at call time.
- RARA repository maintenance rules belong in this file, specs, or skills instead of being added to every runtime prompt.
- Keep prompt-section order stable. New prompt material should prefer additive sections near related guidance instead of reordering existing sections, to preserve provider cache-prefix stability.

## 4. Current Key Decisions

1. Local model support uses Hugging Face `candle` from the upstream `main` branch.
2. Local model loading is provider-agnostic at the CLI level and resolved through model presets and aliases.
3. Agent/tool integration for local models currently uses a constrained JSON tool-calling shim instead of model-native function-calling.
4. Model downloads use a persistent cache directory under the user cache root, overrideable by environment variable.
5. The existing TUI setup screen is transitional; model/config changes should move toward inline command-driven interactions.

## 5. Documentation Rules

- RARA follows `Specification-Driven Development (SDD)` for non-trivial work:
  - define or update the relevant behavior/specification first;
  - derive an implementation plan and concrete task breakdown from that specification;
  - align implementation against that specification;
  - record the resulting implementation checkpoint and any remaining follow-up work.
- **Every non-trivial change must leave documentation.** No implementation PR
  that touches behavior, contracts, or architecture should be merged without
  the corresponding spec or journal update.
- `docs/features/` stores stable engineering specs and contracts.
- `docs/journal/` stores dated implementation notes and checkpoints.
- `docs/todo.md` stores active follow-up work only.
- Non-trivial changes MUST update:
  - the relevant feature spec when a contract or behavior changed;
  - a dated journal note for the implementation checkpoint;
  - `docs/todo.md` only when open follow-up work remains.
- When creating or revising feature specs, follow the patterns in existing
  `docs/features/*.md` files (motivation, design, integration points, TUI
  display if applicable, verification).
- Dated journal notes (`docs/journal/YYYY-MM-DD-<slug>.md`) should include:
  what was built, why, what trade-offs were made, and what remains.

## 6. Near-Term Focus

- Inline TUI command surfaces such as `/help`, `/model`, and `/status`.
- Better onboarding and runtime status transparency.
- Stronger local-model prompt formatting and stop-sequence handling.
- A real embedding backend for local memory retrieval quality.

## 7. Repository Skills

Repo-local skills live under `.agents/skills/`. Use them as focused workflow
guides instead of expanding this charter with long procedural detail:

- `add_tests`: choose and write focused RARA test coverage.
- `context-cache-management`: update context compression, memory placement, or
  provider cache behavior while preserving stable prompt prefixes.
- `support-acp`: integrate or debug ACP clients and control-plane behavior.
- `rara-docs-journal`: write or review dated implementation journals under
  `docs/journal/`.
- `rara-docs-spec`: write or review canonical feature specs under
  `docs/features/`.
- `project-github-titles`: write RARA commit and PR titles using the allowed
  project title format.

## 8. Commit Rules

- Always run `cargo fmt` before every commit to ensure consistent formatting.
- Use short project-specific conventional commit titles. This is an intentional
  subset of Conventional Commits, not the full upstream type set.
- Allowed commit title types:
  - `feat`: user-visible feature or capability.
  - `fix`: bug fix or behavior correction.
  - `chore`: maintenance, dependency, tooling, or non-user-facing cleanup.
  - `test`: test-only changes.
- Format commit titles as `type: subject` or `type(scope): subject`.
- Do not use `!` breaking-change markers in commit titles; describe unusual
  compatibility impact in the PR body instead.
- Keep the subject concise, imperative, and lowercase unless a proper noun or
  code identifier requires otherwise.
- Do not use unlisted types such as `docs`, `refactor`, or `style`; fold those
  changes into the closest allowed type. Documentation-only and spec-only
  changes should use `chore` unless they are part of a user-visible `feat` or
  behavior `fix`.
