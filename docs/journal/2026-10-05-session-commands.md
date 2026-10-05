# Session Commands

## Background

Issue #990 requests six commands: copy, new, diff, init, rename, and export.
Existing `/clear` only resets presentation. `CreateSession` is not handled by
the TUI runtime processor, and thread metadata has no persistent title.

References inspected before implementation:

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`: `slash_command.rs`,
  `chatwidget/slash_dispatch.rs`, `app/event_dispatch.rs`. Adapt typed new-session
  routing, busy-command admission, asynchronous Git capture, ordinary `/init`
  prompt submission, and copying the last assistant Markdown response.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `commands/clear/conversation.ts`, `commands/rename/rename.ts`,
  `commands/copy/copy.tsx`, `commands/export/export.tsx`, and `commands/init.ts`.
  Adapt new identity with explicit state reset, durable custom titles, Markdown
  code-block extraction, and conversation export. Preserve this project's
  `/clear` contract; do not copy synchronous export writes or process-global
  session state.

## Plan And Phase Gates

The branch is stacked on #1029 to reuse ordered background thread persistence.
The existing metadata, runtime client, Git capture, clipboard, and Markdown
export boundaries provide the implementation path. Local edits, isolated tests,
Git branch publication, and PR creation are authorized by the active issue goal.
No destructive migration, history rewrite, or Bazel configuration change is
needed. The optional title is an additive, backward-compatible metadata/index
extension using the existing schema upgrade mechanism.

1. **Runtime identity and names.** Add title persistence and runtime-owned
   new-thread preparation/reset. Preserve the old thread until durable
   preparation succeeds. Verify legacy data, failure retention, resumed titles,
   and isolation of history, goal, plan, interaction, and queue state.
2. **Copy and init.** Wire palette/help/dispatch to the existing clipboard and
   typed prompt routes. Verify exact clipboard content, Markdown code extraction,
   normal permissions, unsupported arguments, and busy admission.
3. **Diff and export.** Reuse asynchronous Git capture and ordered thread reads.
   Render bounded scrolling; atomically export complete Markdown/JSON without
   overwriting files. Verify late results, errors, resize, export parity, and
   storage barriers, then run the complete affected suites and strict Clippy.

## Implementation

- Registered all six commands in palette/help, busy admission, and production
  Enter dispatch. `/init` submits an ordinary repository-inspection prompt;
  `/copy` uses the existing clipboard owner and the Markdown parser for code.
- Added an optional canonical/index title with an additive SQLite upgrade.
  Metadata/index mutations hold a bounded advisory lock so a later checkpoint
  cannot erase a concurrent rename. Named empty threads can be resumed.
- Added ordered storage commands that survive a dropped UI receipt. New-thread
  preparation waits for preceding writes, creates a fresh record, and prepares
  goal persistence before applying an infallible runtime reset. Failures leave
  the current agent available. Old goal records remain unchanged; continuation
  tickets, interactions, plans, counters, retrieval, and private turn state reset.
  Stable workspace services, permissions, model/context budgets, and shared task
  selection survive. The cached extension projection survives without another
  filesystem discovery pass. Existing queued prompts gate `/new`; prompts queued during
  preparation execute after the new runtime root identity is installed.
- Reused review's bounded asynchronous Git capture for a full-screen, wrapped
  diff overlay. Numeric scroll state is independent of render objects and clamps
  on input and resize. Closing the overlay aborts capture; late results cannot
  reopen it. Git errors stay distinct from a clean tree.
- Added atomic Markdown/JSON exports with no-overwrite publication and explicit
  source-corruption errors. Export uses durable display turns when available,
  including pre-compaction content and turns hidden by `/clear`. Legacy message
  rendering excludes model-only blocks and hidden system/developer messages.
  The export worker uses `tempfile`,
  promoted from the existing development dependency without a version change.
- Fixed a prerequisite found while testing export: clearing the UI reset turn
  ordinals and allowed later commits to replace earlier turns. Ordinals now
  advance independently of displayed turns and resume from the highest stored
  ordinal, including sparse records. Checkpoints also persist the actual agent
  execution mode instead of hardcoding execute.
- Split the existing oversized thread-store test file into focused modules and
  extracted state-db thread-index queries to keep touched source files below
  the repository's file-size limit.

## Validation

- Full library suite: 1956 passed, 5 ignored. The first sandboxed run exposed
  existing tests requiring local sockets and writable user runtime storage;
  the same suite passed with those permissions.
- Persistence/state crates: 9 and 10 tests passed, respectively; doc-tests passed.
- Ordered storage regressions: 7 passed, including dropped receipts and failed
  preceding writes. Title regressions include legacy schema upgrade, restart,
  validation, named empty threads, and checkpoint preservation.
- Production command tests cover new-thread state/configuration isolation,
  failed preparation, queued prompts, ordinary init execution, copy payloads,
  busy/usage errors, export ordering/no-overwrite/hidden context/corruption, and
  diff capture, navigation, resize, and dismissal. The full-screen diff snapshot
  was inspected before acceptance.
- Follow-up focused checks cover custom-title resume search and rendering,
  cached extension counts after `/new`, and hidden legacy instruction filtering.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` passed.
- `cargo fmt --all` and `git diff --check` are the final formatting checks.

The targeted Nowledge retrieval is unavailable with
`space_client_upgrade_required` / `exact-v1`; credentials and the current Space
are unchanged. This journal retains the implementation decisions.

## Follow-Ups

No implementation follow-up is deferred from #990. Default remote CI remains
the publication gate, including Bazel; the branch depends on #1029's storage
worker and does not alter Bazel configuration.

## Updated Storage Base Integration

Preserved both diff-overlay navigation and bounded read-only overlay navigation
when merging the updated background-storage base. Command admission still
distinguishes review preparation from an active thread command. Clipboard and
thread-command feedback use the shared typed notice path, preserving severity,
redaction, expiry, and transcript recording. Clipboard wording refers to copied
text because both transcript selections and `/copy` use the same owner.

Integrated TUI validation passed 1,110 tests, with seven parent-driven child
fixture entry points ignored. Existing command and clipboard tests now observe
the typed notice owner; the clipboard matrix still verifies severity.
