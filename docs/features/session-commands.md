# Session Commands

## Problem

Routine thread operations must be available from the composer without confusing
local transcript cleanup with a durable runtime session transition.

## Scope

`/copy`, `/new`, `/diff`, `/init`, `/rename`, and `/export` share the command
palette, help, production key dispatch, and notice/error surfaces.

## Non-Goals

These commands do not implement conversation forks, rewind, configurable
keymaps, provider changes, automatic naming, or an external-client protocol
migration. `/clear` keeps its existing presentation-only behavior.

## Architecture

The TUI parses commands and renders results. Runtime session operations own
identity, agent-state reset, naming, and durable thread data. Filesystem work
uses the existing ordered background storage boundary; Git capture and clipboard
helpers remain asynchronous. No command may block the input loop on disk or Git.

## Contracts

### New Thread

- `/new` creates a new durable thread ID using the current workspace, provider,
  model, permissions, instruction configuration, and shared task-list selection.
- Reject the command while a runtime task, restore, or active child agent is
  running. A pending interaction may be left with its old thread; it must never
  be answered or inherited by the new thread.
- Finish pending old-thread writes before creating the new record. Only switch
  runtime identity and clear the visible transcript after preparation succeeds.
  Failures leave the old runtime and conversation available.
- New history, compaction, token counters, plan/todo state, pending/completed
  interactions, retrieved context, turn IDs, and goal continuation tickets start
  empty. The old thread remains resumable, including its persisted goal and plan.
  A goal is not copied, completed, or implicitly paused by switching threads.
- Preserve prompt recall and the current configuration. A draft typed after the
  command was dispatched must not be overwritten by its delayed completion.
- Existing queued prompts must be sent or removed before `/new`. Prompts queued
  after preparation starts belong to the new thread after a successful switch.

### Rename

- `/rename <name>` assigns an explicit nonempty thread title, limited to 256
  Unicode scalar values with no control characters. Missing/invalid arguments
  report usage without changing the existing name.
- Names are durable runtime metadata, reflected in the resume picker and its
  search, thread inspection, and export. A normal checkpoint, resume, or backend
  rebuild cannot erase a name. Older metadata without a title remains readable.
- The canonical metadata file carries the optional title; the SQLite session
  index stores it for listing. Failure never reports a successful rename.
- A named empty thread is eligible for the resume picker.
- Title search uses the full index within the selected scope before page limits;
  a title on an older thread remains searchable beyond the initial result page.

### Copy

- `/copy` copies the last completed assistant response as Markdown through the
  existing OSC 52/native clipboard path. `/copy code` copies the last fenced or
  indented code block in that response using the Markdown parser.
- User text, reasoning, tool results, notices, and an unfinished streamed
  response are not fallback assistant answers. Missing answers/code blocks and
  unsupported arguments produce notices without altering the clipboard.
- Copy remains available while a later task is running and preserves its state.

### Init

- `/init` submits a normal agent request to inspect the repository and create or
  update `AGENTS.md`. Existing instructions must be read and preserved; the
  request asks for concise, repository-supported guidance rather than a template
  overwrite. Current permissions and plan mode still apply.
- The command does not write files directly or bypass approval. Arguments are
  rejected with usage, and a running task must finish or be cancelled first.

### Diff

- `/diff` opens a scrollable, wrapped overlay containing staged and unstaged
  working-tree changes from the same safe Git capture used by `/review`.
- Include headings and an explicit size-limit notice when bounded capture is
  truncated. A clean tree, a non-repository, a timeout, and a Git failure have
  distinct outcomes. Git errors must never look like a clean tree.
- Up/Down, PageUp/PageDown, Home/End, and the mouse wheel scroll the overlay.
  Esc/Ctrl+C close it without cancelling underlying work or changing the draft.
  Late capture completion cannot reopen a dismissed overlay.

### Export

- `/export [path]` exports the current durable conversation, including material
  hidden by `/clear`, after preceding storage writes finish. It is unavailable
  during runtime execution or a thread transition so the export has a coherent
  boundary.
- `.json` selects a versioned JSON document; `.md` or no extension selects
  Markdown. Other extensions report usage. An omitted path creates a unique
  `conversation-<thread-id>-<timestamp>.md` in the workspace.
- JSON contains thread metadata and the same chronological conversation surface
  as Markdown. Model-only context and hidden prompt material are excluded.
- Resolve relative paths against the session workspace. Write atomically using
  a temporary file in the destination directory, refuse existing destinations,
  and report the final path or actionable error. Do not silently truncate data.
- Clearing the display keeps the durable turn ordinal increasing. Export uses
  committed display turns when available, retaining pre-compaction history;
  legacy threads without display turns use their stored message history.
  Corrupt committed turn records fail export with a source location.

## Validation Matrix

| Contract | Evidence |
| --- | --- |
| Discoverability and ownership | Palette/help entries, usage errors, production Enter dispatch |
| New thread | Runtime ID changes, old thread resumes, goal/plan/input/cache isolation, failed preparation retains old identity |
| Rename | Legacy schema/metadata reads, restart, later checkpoint, resume search, empty named thread |
| Copy/init | Captured clipboard payload and typed runtime prompt through real dispatch |
| Diff | Temporary Git repositories, failures/clean distinction, reviewed overlay and scroll bounds, late-read dismissal |
| Export | Ordered pending writes, Markdown/JSON parity, hidden context exclusion, no overwrite, filesystem failures |

## Source Journals

- [Session command implementation](../journal/2026-10-05-session-commands.md)
