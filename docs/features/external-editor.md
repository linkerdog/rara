# External Composer Editor

## Problem And Scope

Ctrl+G edits the current composer draft in the user's configured editor. The
terminal, input reader, original draft, and temporary file each have explicit
ownership. Editor failure must not consume the draft or leave terminal modes
owned by the wrong process.

## Contracts

- Ctrl+G is available from the composer, including slash/file completion, when
  no pending decision owns input. Other overlays ignore it. Key repeats cannot
  start another editor. Pending paste is flushed before capturing the draft.
- Prefer a nonempty `VISUAL`, then `EDITOR`. Parse quoted executable/arguments
  into argv using POSIX quoting, without shell evaluation, and append the path as one
  argument. Missing/invalid configuration produces a notice. GUI editors must
  be configured with their wait flag, for example `code --wait`.
- Write expanded owned paste content and canonical file references to a private
  temporary Markdown file. The live draft and payload ownership remain intact
  until successful readback. Unchanged content preserves the original cursor
  and collapsed paste representation. Edited content uses the normal paste
  sanitization boundary and returns to the composer without submitting.
- Release the terminal event reader, finish the inline frame, and restore TUI
  modes before launching the editor with inherited terminal streams. On Unix,
  preserve cooked termios across editors that exit without cleaning up; Ctrl+C
  belongs to the editor rather than terminating the parent TUI. Preserve the
  shell title across editors that change it, and restore cooked modes, title,
  and the primary screen if the editor future is canceled.
- Reacquire TUI modes and a fresh event reader after success, spawn/wait failure,
  nonzero exit, or readback failure. Repaint at the current terminal size and
  invalidate cached terminal feedback. Restoration failure is fatal and must
  surface rather than continuing with an unusable terminal.
- While editing, runtime events continue to be projected, but TUI rendering,
  terminal feedback, input polling, and mode maintenance are suspended.
- Session/workspace/draft ownership fences successful readback. If another
  source replaces the draft during editing, preserve that new draft and save
  the edited text to a private recovery file with a visible path.
- Normal temporary files and editor backup artifacts are removed on all paths.
  Recovery files are retained deliberately for the user. Cleanup failures are
  reported with their path. No editor text is sent to the model automatically.

## Architecture

`external_editor` owns command resolution, private file preparation/readback,
process lifecycle, draft capture/application, and terminal handoff. The live
`EventSource` adapter owns the event reader and mode guard. The event loop
pauses presentation while pumping ordinary runtime activity until editing ends.
Existing suspend, title, keyboard, and panic-cleanup contracts remain in their
own modules.

## Validation Matrix

| Boundary | Evidence |
| --- | --- |
| Configuration | VISUAL precedence, blank fallback, quoting, literal shell syntax |
| File transaction | Expanded paste seed, success, nonzero/spawn/read failure, cleanup |
| Draft | Failure/unchanged preservation, edited replacement, stale owner recovery |
| Key ownership | Composer/completion, overlays, pending decisions, repeats |
| Runtime | Events drain during editing; no TUI output until terminal reacquisition |
| Terminal | PTY cooked/raw modes, input ownership, resize, Ctrl+C, cancellation, title balance, error recovery |

## Non-Goals And Risks

No editor auto-detection, automatic GUI detachment, file-content attachment, or
runtime protocol changes. A configured editor must remain running until its
file is saved. Cooperative process cleanup cannot recover from a killed parent
or terminal device disappearance. PTY acceptance is Unix-based; native Windows
terminal handoff is not covered by these fixtures.

## Source Journals

- [Implementation checkpoint](../journal/2026-10-05-external-editor.md)
