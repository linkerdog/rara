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
- [7. Commit Rules](#7-commit-rules)

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
- `docs/features/` stores stable engineering specs and contracts.
- `docs/journal/` stores dated implementation notes and checkpoints.
- `docs/todo.md` stores active follow-up work only.
- Non-trivial changes should update:
  - the relevant feature spec when a contract or behavior changed;
  - a dated journal note for the implementation checkpoint;
  - `docs/todo.md` only when open follow-up work remains.

## 6. Near-Term Focus

- Inline TUI command surfaces such as `/help`, `/model`, and `/status`.
- Better onboarding and runtime status transparency.
- Stronger local-model prompt formatting and stop-sequence handling.
- A real embedding backend for local memory retrieval quality.

## 7. Commit Rules

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
