# 2026-05-13 Claude Prompt Locality

## Summary

Migrated a small Claude prompt slice into RARA by adding execute-only
task-tracking guidance and documenting prompt locality as a first-class runtime
contract.

## Background

RARA's default prompt already covered most of the obvious Claude/Codex
engineering guidance. The remaining gap was narrower:

- execute-only workflow guidance such as mutable todo tracking was not
  expressed as a mode-local rule;
- prompt locality existed as an implicit preference, but not yet as an explicit
  spec contract.

## Scope

- Added an execute-only dynamic prompt section that tells the model to use
  `todo_write` for complex multi-step execution and keep that state current.
- Updated the prompt-runtime spec to describe locality and rule placement more
  explicitly.

## Key Decisions

- Keep mutable execution workflow rules in an `execute_mode` addendum instead of
  the shared base prompt so plan/review turns stay narrower.
- Treat locality as part of cache-stable prompt architecture: preserve section
  order, keep dynamic rules behind the existing boundary, and avoid copying
  provider-specific prompt wrappers from Claude Code.

## Validation

- `cargo fmt`
- `cargo test -p rara-instructions`

## Follow-Ups

- No new follow-up was opened by this slice.
