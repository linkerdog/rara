# Persistent Prompt History

## Background

Issue #989 requires bounded cross-session recall and Ctrl+R history search.
Existing history is a 200-entry in-memory vector; submit trims text and expands
large pastes before recording, so privacy decisions need to move earlier.

Reference implementations inspected before coding:

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`:
  `message-history/src/lib.rs`, `tui/src/bottom_pane/chat_composer_history.rs`.
  Adapt bounded JSONL, locked background writes, lazy reads, stale-response
  rejection, and a separate newest-first search state. Do not copy its claim
  that PIPE_BUF guarantees atomic regular-file appends; serialize writers.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `src/history.ts`, `src/components/HistorySearchDialog.tsx`. Adapt owner-only
  file creation, pending/local history integration, cancellable loading, and
  separate query/preview state. Do not persist attachment or paste payloads.
- Existing background-storage work in #1029 supplies the ordered worker and
  shutdown pattern. History remains independent of StateDb and that branch.

## Plan And Phase Gates

1. **Store:** add the bounded JSONL contract and privacy filter in the existing
   persistence crate. Use a separate lock inode so compaction cannot split writer
   ownership. Exit after restart, contention, rotation, corruption, and privacy
   tests establish the storage boundary.
2. **I/O integration:** add a lazy bounded worker and a disable setting; capture
   original input and paste ownership before transformations. Fence late reads
   and drain writes on shutdown. Exit after real submit and delayed-I/O tests.
3. **Search:** add an isolated query editor and result preview. Keep the composer
   draft intact until explicit acceptance. Exit after key-dispatch, Unicode,
   cancellation, rendering, full TUI, formatting, and strict Clippy validation.

No unresolved product decision blocks implementation. Defaults follow the
existing 200-entry recall limit; size bounds and a persistence switch are
specified in the canonical feature document. Local source edits, test execution,
and an independent review branch are already authorized. No Bazel configuration
changes or external message delivery are required.

## Implementation

The existing persistence crate now owns a 200-entry, 1 MiB JSONL history with
16 KiB prompts, stable UUIDs, owner-only Unix files, and shared secret redaction.
Writers serialize through a separate lock inode so atomic compaction cannot
create independent lock owners. Bounded lock-free reads retain complete records,
report malformed-record counts, and never expose raw parse diagnostics. The byte
limit includes the actual existing file size, including noncanonical whitespace.

The TUI binds history to the config home only at production session startup.
Constructors and render harnesses remain I/O-free. A bounded worker queues
redacted writes and lazy reads; a read follows preceding writes and reports
pending IDs so failed or newer local submissions survive snapshot merging.
Leading-space, oversized, and owned large-paste submissions are excluded before
retention; dispatch still receives the original submitted content. Disabling
persistence preserves local recall and leaves existing history files untouched.

Ctrl+R opens an independent Unicode query editor with newest-first unique
matches and a multiline preview. Query changes select the newest match; explicit
traversal pins the selected text across a delayed refresh. Acceptance replaces
the composer without submitting. Cancellation preserves the draft, cursor, and
large-paste ownership. Lazy Up/Down tracks pending traversal and rejects stale
draft/cursor/overlay owners; failed reads still allow local recall.

Shutdown restores terminal ownership before joining the writer, including error
paths. A flush failure carries history-specific context and does not skip the
normal memory-sync drain. The feature remains independent of #1029; integration
must preserve both ordered storage drains and both event-loop poll sites.

## Validation

- Old-code key-dispatch replay reproduced Ctrl+R inserting `r` into the original
  draft instead of opening search. The same regression now passes.
- Persistence coverage verifies restart/idempotence, bounded count/bytes,
  redaction, torn-tail and malformed-record recovery, lock-free reads, bounded
  contention, and four independent writers during compaction.
- Eleven TUI history tests cover real submit dispatch, disabled persistence with
  existing and absent files, cross-session refresh, stale reads, pending Up/Down,
  explicit search selection, new local submissions, failed-write retry, Unicode
  query editing, accept/cancel, paste ownership, and narrow cursor bounds.
- Reviewed the search snapshot for query/result/preview/footer order; fixed its
  workspace label to keep it independent of the checkout path.
- The full production PTY session holds a writer lock through terminal
  restoration, then checks `/quit` survives immediate exit. A second fixture
  makes the history path unavailable and verifies the error after restoration.
- `cargo test --locked --lib tui::`: 1057 passed, 4 isolated fixtures ignored.
  Parent tests explicitly invoke the relevant ignored children.
- `cargo test --locked -p rara-persistence -p rara-config`: both crates and
  doctests passed (64 config and 15 persistence tests); the persistence writer
  child is invoked by its parent test.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

## Remaining Boundaries

Pattern redaction cannot detect arbitrary secrets. Locking requires cooperating
writers and a supporting filesystem. Process aborts and uncatchable termination
can lose queued writes or leave a torn append. These are documented limits, not
additional work for this issue. No open implementation follow-up was added.

## Main Integration

The main merge preserves both prompt-history and diagnostic initialization and
polling. History failures and recovery notices use the shared typed warning
path, preserving redaction, expiry, and the single transcript record. History reads and writes remain off the input loop. The separate shared
redaction fix in PR #1048 remains a merge prerequisite for this feature; this
conflict resolution does not duplicate that implementation.

Integrated validation: 1,118 TUI tests passed with seven parent-driven child
fixtures ignored. Eight focused history persistence tests passed with one
parent-driven writer fixture ignored.
