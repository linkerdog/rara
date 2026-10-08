# Prompt History

## Problem

Composer recall must survive application restarts without making input wait for
filesystem operations or retaining known secrets and expanded paste payloads.

## Scope

The local TUI keeps bounded per-user prompt history and incremental reverse
search. The history file is independent of conversation persistence and model
context. Local interaction follows INPUT-02 in the interaction specifications.

## Non-Goals

History is not a conversation transcript, attachment store, shell history,
runtime memory source, or ACP/Wire command. It does not index repository files.
Pattern redaction does not guarantee detection of arbitrary secrets.

## Architecture

- `rara-persistence` owns `prompt_history.jsonl` and a separate stable lock file
  beneath the configured application home (the config file's parent directory).
- A private, lazily started TUI worker serializes history operations away from
  the input task. Reads never acquire the cross-process write lock.
- The composer owns local recall and search state; renderers consume that state
  without owning persistence or input editing.

## Contracts

- `tui.history.enabled` defaults to `true`. When disabled, history performs no
  disk reads or writes; bounded session-local recall and search still work.
  Set `"tui": {"history": {"enabled": false}}` in the application configuration
  before starting the TUI to disable persistence without removing existing data.
- History retains at most 200 entries and 1 MiB of serialized JSONL. Each prompt
  is limited to 16 KiB before and after redaction. Oversized prompts are skipped,
  never silently truncated into a different command.
- Filter before trimming or expanding paste placeholders. Leading ASCII space,
  empty input, and submissions containing owned large-paste payloads are omitted.
  Redact accepted text with the shared persistence redactor before retaining or
  queueing it. The submitted prompt itself is unchanged.
- Each entry has a stable unique ID. Writers use a separate advisory lock across
  append and atomic replacement, including when rotation replaces the data inode.
  The lock has bounded acquisition retries. History files are owner-only on Unix.
- Reads use a bounded file snapshot, ignore an unfinished last line, and recover
  complete valid records from malformed data without logging raw payloads.
  Rotation keeps the newest records that fit both bounds. Repeated writes of an
  existing ID are idempotent.
- First Up recall and each new search read lazily, after preceding local writes.
  Merge the snapshot with newer local submissions by ID. A delayed read must
  not replace a changed draft, cursor, overlay, or pending-interaction owner.
- Ctrl+R searches newest-first using case-insensitive literal matching. Query
  edits restart at the newest match; repeated Ctrl+R/Up and Down traverse unique
  matching text without wrapping. Enter accepts into the composer without
  submitting. Esc/Ctrl+C dismiss search and preserve the original draft, cursor,
  and pending large-paste payloads.
- History failures surface as notices/logs; input remains usable. Normal and
  error exits drain queued writes after restoring terminal ownership. No durable
  guarantee is made for process aborts or uncatchable termination.
  Failed writes remain pending for the next write, refresh, or shutdown attempt;
  bounded queue exhaustion is reported explicitly. History cleanup failure does
  not bypass the normal memory-sync drain.

## Validation Matrix

| Contract | Verification |
| --- | --- |
| Restart and concurrent writers | Independent stores and isolated writer processes sharing a temporary home |
| Append/rotation/read safety | Count/byte limits, idempotence, torn tail, malformed records, lock contention |
| Privacy and disabled setting | Real submit dispatch, placeholder ownership, redacted file bytes, no filesystem creation when disabled |
| Delayed background work | Input stays responsive; stale reads do not change a new draft or overlay |
| Search ownership | Production key dispatch, Unicode editing, paste, accept/cancel, newest-first unique traversal |
| Visible search | Focused buffers/snapshots at normal and narrow sizes, long/multiline previews |
| Shutdown | Queued submission survives immediate normal exit; write errors are observable |

## Open Risks

Advisory locking requires cooperating writers and a filesystem that supports it.
Unexpected process termination can leave an incomplete trailing append, which
the next reader ignores and the next writer repairs.

## Source Journals

- [Persistent prompt history](../journal/2026-10-05-prompt-history.md)
