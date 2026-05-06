# File Search

RARA provides a shared file-search crate for workspace file discovery.
The crate is intentionally separate from model-facing tools so TUI pickers,
context routing, skills, and tool adapters can share the same traversal and
ranking behavior.

## Problem

RARA's file discovery behavior was split across `list_files`, `glob`, and
`grep`, with directory filtering implemented as local hard-coded path checks.
That made ignore behavior less accurate than ripgrep-style traversal and left
no reusable search primitive for TUI file pickers or context file routing.

## Scope

The first implementation introduces a reusable file-search crate and routes the
`list_files` tool through it.

In scope:

- Use git-aware ignore semantics instead of ad hoc directory filtering.
- Keep file discovery bounded and stable for model-facing tool results.
- Provide fuzzy path ranking for future TUI pickers and context file routing.
- Keep RARA-specific policy, such as build-artifact suppression, outside the
  generic crate.

## Non-Goals

- Replace content search in `grep`.
- Add a TUI file picker in the first slice.
- Inject selected files into context automatically.
- Persist file-search indexes.

## Architecture

`crates/file-search` is a pure library crate. It owns traversal, fuzzy matching,
stable ordering, and result metadata. Tool adapters decide which options to use
for their model-facing contracts.

This mirrors Codex's separation between a file-search crate and higher-level UI
or CLI consumers while keeping RARA-specific tool policy out of the shared
crate.

## Contracts

`crates/file-search` exposes:

- `list_files(root, options)` for bounded file listing.
- `search_files(query, roots, options)` for fuzzy path search.
- structured results with relative path, root, match type, total count, and
  truncation status.

Traversal uses the `ignore` crate, following the same family of behavior used
by ripgrep. Fuzzy path ranking uses `nucleo`.

The crate defaults to:

- respecting git ignore files;
- applying `.gitignore` only in a git context;
- including hidden entries;
- following symlinks;
- returning stable path ordering for plain listing;
- sorting fuzzy matches by score descending, then path ascending.

### Tool Adapter Policy

The `list_files` tool uses the shared crate but preserves RARA's model-facing
contract:

- output remains a `files` array;
- results are bounded by `limit`;
- `total_count` and `truncated` report omitted entries;
- `include_ignored` disables both gitignore handling and RARA build-artifact
  suppression.

The tool adapter owns RARA-specific default excludes such as `target`,
`node_modules`, `dist`, and virtual environments. Those excludes do not belong
in the generic crate because other callers may need exact git-aware traversal.

## Validation Matrix

- `.gitignore` in a git workspace suppresses ignored files.
- `respect_gitignore = false` includes ignored entries.
- plain listing is stable and bounded.
- fuzzy search ranks matching paths.
- `list_files` still suppresses build artifacts by default.
- `list_files include_ignored=true` includes those artifacts.
- `list_files limit` reports `total_count` and `truncated`.

## Open Risks

- The current crate performs a fresh walk per call. Large-workspace TUI pickers
  should use a session-style incremental search surface instead.
- `glob` and `grep` still have their own traversal paths. They should move to
  the shared crate only when their output contracts and ignore semantics are
  updated deliberately.
- File search is not yet connected to `MemorySelection`; automatic context
  injection must preserve budget and cache-prefix stability.

## Follow-Up

Future work should add a session-style incremental search surface for TUI
pickers, then use the same crate for context file routing before injecting
candidate files into `MemorySelection`.

## Source Journals

- `docs/journal/2026-05-07-file-search-crate.md`
