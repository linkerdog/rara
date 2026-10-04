# Composer File Mentions

## Problem

Users need to name workspace files without blocking input on directory traversal
or breaking a reference while editing a draft.

## Scope

An `@` query at a composer token boundary opens a fuzzy file picker. Selection
inserts an atomic file reference. File references and owned large-paste markers
share cursor, deletion and wrapping boundaries. References survive ordinary
history navigation, Ctrl+R and persistent text history.

## Non-Goals

Automatic file reads or content attachment, directory references, file watching,
and a new runtime input protocol are outside this change.

## Architecture

The shared `rara-file-search` crate owns cancellable, bounded discovery and fuzzy
ranking. One TUI worker owns one current workspace index and coalesces queries;
the UI sends requests and consumes bounded result snapshots. Rendering performs
no filesystem operations. Search state is scoped to the active draft/session.

Composer atoms retain explicit source ranges. File mentions use an unambiguous
text encoding that can be reconstructed from history; large pastes retain owned
payloads and expand only their actual ranges on submission. Ordinary text that
resembles a paste label does not acquire ownership of a hidden payload.

## Contracts

- `@` at the start of a whitespace-delimited token opens the picker. Email
  addresses and existing encoded mentions do not reopen it. Queries use fuzzy
  workspace-relative path matching. Hidden files follow the shared ignore
  policy; build artifacts and `.git` internals are excluded by the TUI adapter.
- Searches debounce edits and use one worker with a replaceable latest request.
  Session, workspace, query generation and draft/cursor identity fence results.
  Closing the popup or changing workspace cancels obsolete discovery/ranking.
- Discovery retains at most 100,000 files and 32 MiB of path data. Results retain
  at most 50 matches. A truncated index is visible; cancellation is silent and
  actual search failures are visible. Reopening starts a fresh index.
- Tab or Enter accepts the current result and inserts `@` followed by a JSON
  string path, for example `@"src/parser.rs"`. Paths containing spaces, quotes,
  backslashes or Unicode round-trip without ambiguity. Enter during loading or
  with no selection never submits the draft accidentally.
- Esc closes the picker and preserves text/cursor. The same unchanged query
  stays dismissed; a subsequent edit can reopen it. Other overlays and pending
  decisions retain their input priority. Mouse events while completion is open
  cannot select or scroll hidden transcript rows.
- Left/right and backspace/delete cross a whole atom. Vertical cursor placement
  never enters an atom. An atom wraps as one unit; an overwide label is clipped
  for display while its source and submission text remain complete.
- A file reference reaches the model as the encoded inline path in the user
  message. Selection does not read file content, alter system instructions or
  bypass tool permissions. Historical references are reconstructed from the
  encoding without checking current file existence.

## Validation Matrix

| Boundary | Checks |
| --- | --- |
| Shared index | Ignore policy, cancellation, path/count bounds, Unicode ranking |
| Worker | Latest request wins, one worker, stale workspace/session/draft fences |
| Composer | Atom movement/deletion/wrapping, mixed pastes, Unicode boundaries |
| Picker | Trigger, accept, loading/empty/error, Esc, focus/overlay ownership |
| History and submission | Local recall, Ctrl+R, persistent encoding, exact prompt |
| Rendering | Short/narrow viewport snapshots and valid cursor coordinates |

## Open Risks

Filesystem traversal can be delayed by a single OS operation; cancellation is
cooperative and the UI never joins a scan synchronously. Index bounds can omit
files in very large workspaces and must be reported in the picker.

## Source Journals

- [Implementation checkpoint](../journal/2026-10-05-file-mentions.md)
