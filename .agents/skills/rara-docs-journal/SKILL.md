---
name: rara-docs-journal
description: Use when creating, updating, or reviewing RARA implementation journals under docs/journal/. Applies to dated rollout notes, implementation checkpoints, validation evidence, and deciding what belongs in a journal versus a feature spec or docs/todo.md.
---

# RARA Journal Writing

Use this skill when writing or reviewing `docs/journal/YYYY-MM-DD-topic.md`
files in this repository.

## Goal

Keep journals as concise implementation records:

- what changed
- why it changed
- what was validated
- what remains open

Journals are not canonical specs and should not become scratch-note dumps.

## Surface Rules

- Journal files live under `docs/journal/`.
- Filenames use `YYYY-MM-DD-topic.md`.
- Use one topic per file; append to an existing journal when the follow-up is
  part of the same rollout.
- Stable contracts belong in `docs/features/`.
- Open follow-up work belongs in `docs/todo.md`.

## Structure

A normal journal should use these sections when they fit:

- `Summary`
- `Background`
- `Scope`
- `Key Decisions`
- `Validation`
- `Follow-Ups`

For small checkpoints, shorter headings are fine if the entry still records the
post-change truth and the remaining work.

## Writing Rules

### Record Post-Change Truth

Do not leave pre-fix wording in place after the same PR already changed the
code or docs. If a step landed, mark it as done or remove it from remaining
work.

### Keep TODO And Journal Aligned

If `docs/todo.md` references a journal decision, both files must agree on:

- scope
- priority
- whether the item is done or still open
- any effort or rollout estimate

### Make Validation Concrete

List commands or checks that support the entry.

Good examples:

```bash
cargo test tui::render::tests::bottom_pane_grows_for_multiline_input -- --nocapture
cargo check
gh pr checks 395 | cat
```

Avoid vague validation such as `tested locally` or `CI should pass`.

### Keep Follow-Ups Narrow

Follow-ups should describe remaining work only. If a broad item was partly
completed, rewrite the follow-up as the unresolved tail instead of repeating the
old broad task.

## Before Finishing

Check:

- filename date matches the implementation date
- stable contract changes also updated `docs/features/`
- open work is visible in `docs/todo.md`
- validation commands are explicit
- wording matches the actual post-change state
