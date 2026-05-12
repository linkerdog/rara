# 2026-05-12 Claude Prompt Contracts

## Summary

Migrated the first small Claude Code prompt-contract slice into RARA without
copying provider-specific prompt text or changing prompt section order.

## Background

The Claude Code source and extracted prompt references use the same broad
static-versus-dynamic prompt split that RARA already has. The useful migration
surface for this slice was therefore behavioral:

- stronger skill invocation rules at the point where the model chooses whether
  to load a skill;
- a continuation-oriented compact summary schema that preserves enough state to
  resume after history replacement.

## Scope

- Strengthened the `skill` tool description and dynamic skills prompt section
  so matching visible skills are loaded before task-specific work.
- Tightened the skill-name boundary: only exact listed skills or user-typed
  slash shorthand should be invoked.
- Updated the default compact instruction to preserve completed work, decisions,
  failed approaches, pending interactions, and the next concrete action.
- Narrowed the shell `rg` guidance so it applies only when a dedicated search or
  file-discovery tool is unavailable or unsuitable.

## Key Decisions

- RARA keeps its existing prompt section order and dynamic boundary.
- The compact summary remains markdown-only. It does not adopt Claude-specific
  wrapper tags because RARA does not currently parse those tags.
- Plan-mode phase discipline and richer environment snapshots remain separate
  follow-up work to avoid mixing multiple behavior changes into one prompt PR.

## Validation

- `cargo fmt`
- `cargo test -p rara-instructions`
- `cargo test --bin rara -- tools::skill` was attempted after clearing
  `target/`, but the full binary dependency build exhausted the remaining local
  disk space while compiling `lance-index` and `candle-transformers`.

## Follow-Ups

- Adapt the Claude-style plan-mode phase discipline to RARA's existing
  `exit_plan_mode` contract.
- Consider a dynamic environment/status snapshot section that labels git status
  as a point-in-time snapshot.
