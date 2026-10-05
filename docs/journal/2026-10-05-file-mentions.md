# Composer File Mentions

## Scope And Plan

Issue #992 is based on #1040 so file references can be checked against persistent
history and Ctrl+R. No other open PR is merged by this work.

1. Build a bounded cancellable index in the existing file-search crate. Enter
   with its current ignore/ranking contracts; exit with focused index tests.
   Reusing the index avoids a walk per edit. Bounds and cancellation prevent
   unbounded retention and obsolete queued scans.
2. Add composer atoms for encoded file references and owned paste ranges. Enter
   with existing grapheme editing and shared wrapping; exit with Unicode,
   deletion, paste ownership and history round-trip checks. Preserve the string
   user-input protocol rather than adding content injection or a new payload.
3. Connect one background worker and the file popup. Enter after model/index
   boundaries are verified; exit with stale-result, key-routing, submission and
   render tests plus strict Clippy/format checks. The worker must never own the
   terminal or block the event loop on a filesystem operation.

Workspace edits, local validation and publishing the resulting PR are covered
by the ongoing issue-queue task. No new external service or permissions are
needed; terminal/socket validation uses the existing approved test scope.

## Reference Adaptation

Local Codex `file_search.rs` and `bottom_pane/file_search_popup.rs` separate
search-session ownership and popup state, retaining query/session generations.
`bottom_pane/chat_composer.rs` uses atomic text elements for file references and
large pastes; `mention_codec.rs` reconstructs linked elements from text history.
Local Claude Code `hooks/fileSuggestions.ts` caches discovery and discards
background results from older generations. Adapt these patterns to the existing
shared Rust file-search crate and the current composer/history boundaries.

Use explicit inline paths rather than automatic file reads: references belong
to the volatile user message, while runtime file tools retain read permissions
and content budgeting. Stable prompt prefixes remain unchanged.

## Implementation

- `FileSearchIndex` shares ignore setup with existing search APIs, caps discovery
  by file count and retained path bytes, reports non-UTF-8 omissions, and ranks
  a bounded Top-K heap. Query changes cancel scoring while retaining discovery;
  closing or changing workspace invalidates the index with a separate epoch.
- One worker per TUI holds replaceable request/result slots. The terminal loop
  polls completion, debounces at 150 ms, and checks generation plus draft,
  cursor, session, workspace, overlay, and pending-decision ownership.
- `composer_atoms` reconstructs JSON-quoted inline references and owns explicit
  paste ranges. Editing rebases ranges, consumes whole atoms, and repairs
  grapheme boundaries. Submission expands ranges from the end so duplicate
  labels and payload text cannot trigger extra replacements.
- Shared wrapping moves atoms as a unit, clips overwide display labels, and
  maps vertical navigation only to atom/grapheme boundaries. Bidi annotations
  retain source offsets. History stores the same inline text; returning from
  Up/Down restores the current draft's owned paste payloads.
- Completion consumes Enter/Tab even while loading or empty. Esc/Ctrl+C keep
  the draft, Ctrl+R transfers focus, and mouse input cannot reach transcript
  rows hidden by the picker. Paste is flushed before completion key routing.

The selected trade-off is snapshot discovery rather than file watching or
content attachments. Reopening refreshes the snapshot; bounded discovery can
omit files and shows an explicit index-limit label. A blocking OS filesystem
operation may delay worker shutdown, but the terminal task never joins it.

## Validation

Validation evidence:

- `cargo test --locked -p rara-file-search`: 9 passed.
- `cargo test --locked --lib tui::`: 1,079 passed; four isolated child-fixture
  entry points remain ignored for direct execution and are parent-driven.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check`, `git diff --check`, and touched source-size
  checks: passed; no touched source file exceeds 1,000 lines.
- Local default Bazel remains affected by the previously recorded rules_rust
  cache problem; remote Bazel CI is the acceptance gate. No Bazel configuration
  or cache settings were changed for this work.

The reviewed popup snapshot covers selection,
quoted paths with spaces, bidi annotation, and placement above the composer.
Additional checks cover tiny viewports, atom wrapping at widths 1 through 79,
Unicode deletion, paste ownership, persistent history recall, exact runtime
submission, worker coalescing, refreshed indexes, failures, and cancellation.

For RED evidence, temporarily disabling production file-completion key routing
made `loading_empty_and_dismissed_popups_keep_the_draft` fail with an empty
composer instead of `@`: Enter had incorrectly submitted the draft. The
mutation was restored before the final checks. Full regression also caught a
newly joined flag-grapheme cursor regression; deletion now snaps backward while
insertion snaps forward, preserving the existing Unicode contract.

## Follow-Ups

No unrelated work is added. A targeted Nowledge lookup failed with
`space_client_upgrade_required` (`exact-v1`); this journal retains decisions.

## Prompt History Base Integration

Merged the updated prompt-history base, retaining both file-search and diagnostic
polling. Paste flushing keeps atomic range edits and owned payloads while
returning notice text to the shared TUI notice owner. Display-boundary coverage
uses the production flush wrapper and the named owned-paste content field.
Search failures now use typed warnings.

Integrated validation: all 1,140 TUI tests passed (seven parent-driven child
fixtures ignored), including atomic paste ownership, notice expiry, display
sanitization, and history recall. All nine file-search tests passed.

The next history-base update (`aa135ea9`, including main `50a6d864`) retains
independent resume/history query editors, atomic composer ranges, storage exit
barriers, and file/history/diagnostic polling. Resolve the interaction-spec
overlap by retaining file completion's ownership of Enter alongside enhanced
Shift+Enter and the legacy Ctrl+J fallback. Remove the superseded risk statement
that paste placeholders are not atomic; that boundary is already covered by
the file-mention implementation and tests.

Integrated TUI validation passed 1,161 tests, with seven parent-driven child
fixtures. The file-search crate implementation is unchanged from its earlier
nine-test validation.
