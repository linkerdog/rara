# Indexed Resume Search And Input Ownership

## Background

Issue #981 combines a misleading footer, duplicate cwd filtering, basename
collisions, newest-200-only search, and missing query editing. This builds on
the asynchronous storage/query owner from #978 (PR #1029).

## References And Staged Plan

Inspected Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
`codex-rs/tui/src/resume_picker.rs`: backend cwd/sort filtering, cursor pages,
deduplication, and pending page-down selection. Inspected Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`,
`src/screens/ResumeConversation.tsx` and `src/components/LogSelector.tsx`:
progressive loading, explicit cross-project selection, and a query editor with
its own cursor. Adapt those boundaries to the SQLite index and existing shared
grapheme editor; push the complete metadata/preview search into SQL.

1. Add a named query/cursor/page API in a narrow state-db module. Preserve
   resumability and the recent-list compatibility wrapper. Exit on SQL tests
   beyond 200 rows, literal queries, full paths, timestamp ties, both sorts,
   and current-session exclusion. No schema changes or filesystem scanning.
2. Integrate bounded asynchronous page reads, explicit scope, initial fallback,
   stale-result rejection, and failure/retry. Exit on isolated storage-backed
   tests for empty cwd fallback, basename collisions, search, and append races.
3. Connect shared cursor editing, truthful shortcuts/footer, visible cursor,
   and page navigation. Split resume rendering out of the large list-picker
   file. Exit on production key/render tests and complete TUI/Clippy checks.

The work is on a separate branch based on #1029 so the pending asynchronous
foundation stays reviewable. Existing local-edit, test, feature-push, and PR
authorization covers these stages. Default remote Bazel remains an acceptance
gate; no build configuration changes are included.

## Decisions

- Automatic scope fallback checks whether a workspace has any resumable rows,
  not whether the current search matches. Explicit toggles never fall back.
- Left/Right edit search. Tab toggles cwd/all, Ctrl+S changes sort, and Ctrl+R
  retries/refreshes. Page navigation loads more results through the same owner.
- Cursor pages use deterministic timestamp/ID ordering; the UI deduplicates
  across live updates. Refresh restarts the listing. Search remains literal
  metadata/latest-preview matching rather than full-history fuzzy search.

## Implementation

- `rara-state` now owns the filtered query, stable cursor, and bounded page API.
  The recent-thread compatibility API delegates to that query. Full-path scope,
  current-session exclusion, and literal search happen before the page limit.
- The asynchronous resume state loads 50 rows at a time, fences results by
  generation and source, and invalidates a previously loaded cursor before
  requesting a page for a changed runtime session or cwd. Existing rows remain
  selectable during append loads; failures appear in the picker and transcript.
- Resume search uses the shared grapheme editor for insertion, deletion, cursor
  movement, and paste. Its renderer scrolls the displayed query by whole
  graphemes and returns the query cursor. Narrow layouts keep shortcut labels
  together, and page navigation uses the measured list capacity.
- Resume rendering is isolated from the generic list-picker implementation.
  The canonical thread and interaction contracts are updated; the completed
  resume-editing item is removed from the remaining keyboard work.

## Validation

- `cargo test --locked --lib tui:: -- --nocapture`: 1064 passed, four existing
  ignored tests. Coverage includes scope fallback, full-path isolation, lazy
  pages, stale requests and loaded cursors, visible failure/retry, shared query
  editing, all footer bindings, and cursor/footer layout at 40/60/80 columns.
- `cargo test --locked -p rara-state`: 13 passed, including three new indexed
  query tests and existing resumability/lineage checks.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

The initial narrow-footer check exposed a split shortcut label. The corrected
layout keeps labels together. Wide-character cursor assertions inspect cells
directly instead of assuming that a buffer's string dump omits continuation
cells. The latest complete TUI run includes the final source-cursor guard.

## Follow-Ups

Required remote CI/review remain merge gates. This PR is stacked on #1029 and
must target `main` after that dependency lands. Local default Bazel has the
previously recorded external `rules_rust` cache failure; its configuration and
cache are unchanged, and the default remote Bazel job remains authoritative.
Physical terminal interaction has not been asserted by these harness tests.
