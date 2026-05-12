---
name: rara-docs-spec
description: Use when creating, revising, or reviewing RARA canonical feature specs under docs/features/. Applies to stable runtime, TUI, protocol, memory, provider, skill, plugin, and tool contracts, including scope boundaries and validation matrices.
---

# RARA Feature Spec Writing

Use this skill when writing or reviewing `docs/features/*.md` files in this
repository.

## Goal

Keep feature specs as the stable source of truth for:

- product and engineering scope
- runtime, TUI, protocol, or tool contracts
- architecture boundaries
- validation expectations
- known operational risks

Feature specs are not date-stamped rollout notes and should not read like a
PR-by-PR changelog.

## Surface Rules

- Canonical specs live under `docs/features/`.
- Filenames are topic-oriented kebab-case.
- Do not use date prefixes in `docs/features/`.
- Chronological implementation notes belong in `docs/journal/`.
- Open rollout tails belong in `docs/todo.md`.

## Structure

Prefer these sections for active specs:

- `Problem`
- `Scope`
- `Non-Goals`
- `Architecture`
- `Contracts`
- `Validation Matrix`
- `Operational Notes`
- `Open Risks`
- `Source Journals`

If a section is intentionally short, keep it short, but do not silently drop
scope, non-goals, contracts, or validation.

## Writing Rules

### Capture Stable Contracts

A spec should answer what behavior is intended, what boundary is canonical, and
what downstream consumers can rely on.

Avoid:

- temporary debugging notes
- CI chatter
- file-by-file implementation inventory with no contract value
- broad brainstorming that has not been accepted as a target

### Keep Scope Explicit

Do not let a spec silently expand into adjacent surfaces. If a capability is
deferred, put it in `Non-Goals` or `Open Risks` instead of implying it is
already part of the contract.

### Distinguish Canonical And Compatibility Paths

When a transition exists, identify the canonical path and the compatibility
path separately. Do not describe both as equally primary unless they truly are.

### Make Validation Product-Aware

The validation matrix should name focused checks that prove the contract:

- Rust unit or integration tests
- TUI render or snapshot tests
- protocol/control-plane tests
- typecheck, lint, or build gates
- browser or MCP verification when the user-visible surface requires it

## Compaction Guidance

When multiple journals describe the same area:

1. Move durable conclusions into the feature spec.
2. Leave rollout evidence and CI notes in journals.
3. Point `docs/todo.md` follow-ups at the canonical spec.
4. Remove or rewrite stale spec text that contradicts the current design.

## Before Finishing

Check:

- filename is topic-oriented, not date-oriented
- scope and non-goals are explicit
- contracts describe the post-change canonical behavior
- validation matrix is concrete
- source journals link implementation evidence
- remaining work is not buried only in prose
