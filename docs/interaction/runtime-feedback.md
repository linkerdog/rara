# Runtime Feedback And Pending Decisions

## Problem

Running work, queued text, pending decisions, and completed output must remain
distinguishable. An accepted command is not proof that a turn stopped or a
decision finished executing.

## Scope And Non-Goals

This document owns visible terminal behavior. Authorization, task execution,
goals, persistence, and model switching remain runtime/feature contracts.
It does not introduce new control-plane messages or concurrency guarantees.

## Architecture

User intents cross `RuntimeClientPort` as typed commands. Runtime snapshots and
events update presentation state through the controller and TUI projection.
The renderer consumes the projection; it does not discover providers or own
runtime extension registries.

## Contracts

### RUN-01: Submission And Queueing

| State | User action | Visible/runtime outcome |
| --- | --- | --- |
| Idle, no pending decision | Submit ordinary text | Start or enqueue runtime input; clear the submitted composer |
| Running turn | Submit ordinary text | Queue a follow-up; keep the current turn and its progress visible |
| Running turn | Submit a slash command | Apply the shared command availability policy; inspection preserves running progress |
| Empty/whitespace input | Enter | No task; lightweight Ready feedback may be shown |
| Pending decision | Choose a displayed option | Send the corresponding typed decision; do not submit the option as a new task |
| Runtime rebuilding | Submit ordinary text | Preserve queued input until the runtime can accept it |

Queue preview and approval information may coexist. A queue indicator must not
hide a pending decision. Queued input is not rendered as a completed response.

### RUN-02: Cancellation Is A Transition

The first Ctrl+C with no overlay requests cancellation of a running turn and
arms the quit shortcut in INPUT-02. Esc does the
same unless it is handling the shell-approval rejection action in RUN-03. The
cancel command, cancellation-requested notice, terminal runtime event, and
task completion are separate states. Do not show successful completion merely
because a cancellation request was sent.

The controller coordinates terminal events with query completion so trailing
events are not lost. A task join failure must surface rather than leave the
presentation waiting indefinitely for an event that can no longer arrive.
Cancellation and interruption retain their typed first accepted stop kind.
An accepted request keeps progress visible while execution drains; terminal
feedback is published only after the task returns. Late output for a terminal
turn, another turn, or another session is ignored before it can reopen a stream
or mark a new query complete. A request after task return is rejected even if
the previous cancellation notice is still visible.

The first accepted stop determines terminal status, even if execution returns a
successful result or an approval while draining. Any underlying failure remains
visible as a diagnostic. A stop after execution has returned is rejected and
must preserve the returned approval. After the query finishes, maintenance
commands such as `/compact` can display their own lifecycle events.

If the broadcast stream lags, retained task events recover the missing tail and
terminal feedback in sequence without duplicating output. A task panic retains
already-produced text, closes the live stream, and surfaces the task failure.
It also clears busy state and pending decisions owned by the lost agent,
without terminating the terminal session. Queued text remains available; the
next submitted prompt uses the existing missing-agent rebuild path. A rebuild
failure remains visible and retryable. The failed agent's in-memory state is
not reused after a panic.

### RUN-03: Approval Focus And Scope

Pending interaction priority is plan approval, shell approval, then requested
input. The visible card, option count, and keyboard mapping must agree.

- Plan approval has three decisions: approve, keep planning, and reject.
- Shell approval has four decisions in visible order: once, reusable prefix,
  always, and reject/suggestion. With no overlay, unmodified Esc selects
  rejection even when the composer contains a draft.
- Empty-composer arrows move selection; Enter applies it. Numeric shortcuts
  select explicit displayed options. Shell approval also accepts F1-F4.
- Nonempty text remains composer input instead of activating navigation letters.
- Request-input cards expose their offered choices and allow the supported
  free-text answer path. Do not imply Enter accepts a default when the request
  contract requires an explicit answer.
- Queue state, stale completed cards, or an unrelated overlay must not grant
  authorization. Approval scope is owned by the runtime policy.
- Pending decisions can use the full terminal viewport. Reserve action rows
  before allocating space to command previews. Long or multiline commands must
  not push choices below the visible panel. Measure visual rows after wrapping,
  and stack choices when they do not fit on one line. Shell details use the
  available height and remain fully reachable with PageUp/PageDown and Home/End
  when the composer is empty. A row-range indicator identifies the visible
  portion. Arrows continue to select actions; paging never authorizes execution.
  The working directory has a fixed summary row when space permits, and its
  complete path also appears in the scrollable details. On very short screens,
  omit decorative header rows before hiding command content or actions.
  Scroll position belongs to the pending tool call, resets for a new call, and
  clamps after resizing. The transcript approval card retains the full command
  and working directory. This layout is shared by local and SSH sessions.

See [planning mode](../features/planning-mode.md),
[shell approval](../features/shell-approval-policy.md), and
[thread goals](../features/thread-goals.md) for the underlying decisions.

### RUN-04: Transcript And Recovery

A failed resume keeps the current session and resume picker available, with a
visible error. Startup resume failure keeps the fresh session available.
Credential synchronization failure keeps the model picker available and does
not start a rebuild. Required resume reads complete before changing session
identity, history, or goal binding. Resume search shows loading state and ignores
outdated query replies. Selected threads load in the background with a persistent
activity indicator; Esc cancels the selection and Enter retains the composer
draft until loading finishes. A successful switch replaces session-local
interactions and keeps live recovery data until its turn is durably committed.

Transcript writes and shared-task scans must not block input or drawing. Quit
shows a saving notice while waiting for the accepted writes; Esc cancels the
exit without cancelling those writes. A failed save keeps the terminal session
open with an error. Final cleanup drains the storage owner before returning.
Workspace context inspection shows loading state until background file inputs
are available; model requests continue assembling current inputs independently.

A successful backend rebuild installs the replacement agent even when saving
configuration fails. The in-session backend remains usable, and a visible
warning explains that the configuration was not saved. Terminal I/O and
transport errors retain their own error contracts.

Render live progress, committed turns, tool lifecycle, thinking visibility,
and pending decisions from typed presentation state. Keep event chronology and
session identity intact. A disconnect is observable; a reconnect alone does
not prove that missed events or the active turn have been restored.

Resume behavior is owned by [threads](../features/threads.md) and
[session transcript](../features/session-transcript.md). Tests must distinguish
restored committed output, restored pending state, and newly running work.

Transcript scrolling is bounded by the currently rendered visual rows. Repeated
Up/PageUp at the top must not delay the next Down/PageDown. Manual upward
navigation anchors the top visual row while streaming appends new rows; scrolling
back to the bottom resumes tail-following. Clear and thread resume start at the
tail. Long histories remain reachable without a 16-bit global-row offset; see
[mouse text selection](../features/mouse-text-selection.md) for shared scrolling,
rendering, and copy behavior.

Presentation changes are applied in event order, while repaint requests are
coalesced at a session-local frame deadline. A final update must become visible
without requiring another event or waiting for the maintenance tick. Input
updates state immediately; resize requests are retained until the next paint
measures current terminal dimensions. Painting may wait one frame interval. See
[streaming transcript](../features/streaming-transcript.md) for scheduling and
the separate incremental-work contracts.

Live Markdown keeps incomplete text replaceable. A table confirmed by
newline-completed source, together with following source, is withheld until
the response's canonical final render. Earlier prose remains visible; event
delivery and transcript chronology are unchanged. Agent and thinking streams
materialize changed source on presentation access and reuse unchanged rows.

Assistant/thinking text is sanitized before source ingestion, with independent
escape and CRLF state across deltas. Logical newlines end unfinished controls
and remain visible, so malformed metadata cannot hide later transcript lines.
Explicit Unicode bidirectional controls appear as `⟦U+XXXX⟧` labels before
Markdown layout. Transcript selection copies those visible labels; raw runtime
and tool payloads retain their original text.
Tool progress keeps a bounded tail per
invocation and stdout/stderr identity, including interleaved same-name calls.
Every progress entry, including its label and truncation marker, is at most
16 KiB and 16 logical lines. Complete terminal-output previews sanitize before
line splitting and retain at most 16 KiB and six nonempty lines. Truncation does
not replace the original runtime/tool artifact. See
[display text boundary](../features/display-text-boundary.md).

### RUN-05: Terminal Lifetime And Restoration

Terminal mode ownership begins before raw mode or input reporting is enabled.
Non-TTY stdout is rejected before ownership or escape-sequence output begins.
Startup failures, event-loop errors, normal exits, and unwinding must restore
raw mode, mouse reporting, bracketed paste, focus reporting, synchronized output,
and cursor visibility. Partial
initialization has the same restoration obligation as a running UI.

Cleanup attempts every owned mode even when one operation fails, preserving
the first cleanup error. An existing startup or runtime error remains the
primary error; a cleanup failure must also surface. A panic hook restores the
terminal and reserves a clean line below the frame before invoking the previous
hook when the TUI owner panics during
initialization or an event-loop poll. Caught background-task panics must not
disable a running UI's terminal modes, including tasks on the same executor
thread between owner polls. If the owner catches a panic after restoration,
the UI must exit instead of continuing with disabled terminal modes.
Restore modes before asynchronous exit work such as memory draining.
Explicit restoration consumes guard ownership even when a cleanup operation
fails; Drop must not repeat that failed cleanup attempt. Signal termination
such as SIGTERM/SIGHUP and non-unwinding aborts are outside this contract.

Only one TUI may own the process terminal at a time. Restoration is idempotent
and does not modify keyboard enhancement stacks that the TUI never enabled.
This contract does not cover uncatchable termination such as SIGKILL. Viewport
and normal shell handoff are covered by RUN-06; Unix job control by RUN-07.

### RUN-06: Viewport Ownership And Shell Handoff

The primary-screen viewport includes both transcript and bottom pane. Allocate
the terminal's available rows once; composer growth changes the internal split,
not the size of the outer viewport. Sidebar width continues to determine the
composer's wrapping width.

Before the first frame, reserve rows below the shell cursor so existing shell
output moves into native scrollback. Do not erase the visible screen or purge
scrollback with ED2/ED3. Relative row reservation does not require a startup
cursor-position query; do not send a redundant DSR probe. Reserve, invalidate,
paint, and place the cursor inside
one synchronized update. Resizing invalidates the owned viewport and repaints
blank cells as well as content; ordinary composer edits do not clear it.

Normal exit places the shell cursor at column zero on a clean line below the
last frame. A terminal at the bottom edge scrolls one line to make room. Error
cleanup attempts the same handoff without hiding the original error. Unwinding
restores modes and hands off a clean line before the previous panic hook emits
diagnostics. Later destructors must not reposition over those diagnostics.
Terminal input modes are restored before asynchronous exit work.

Focus reporting is enabled with the other terminal modes and disabled during
cleanup. Focus gained/lost updates the presentation state before publishing
the next status projection.

### RUN-07: Unix Suspend And Resume

Ctrl+Z yields the foreground process group with SIGTSTP. Before signalling,
stop the input event stream, hand off the inline viewport on a clean line, and
restore all owned terminal modes. Do not stop a job whose terminal cleanup
failed. Signal failures surface and still attempt to reacquire terminal modes.

After the shell resumes the job with `fg`, reacquire terminal modes and the
input stream, then reserve and fully redraw the viewport relative to the
current shell cursor and terminal size. Preserve composer, overlay, transcript,
and running work. Shell output written during suspension stays in native
scrollback. Repeated suspend/resume cycles use the same ownership rules;
temporary restoration must not disable later panic or exit cleanup.

A shell can restore its saved job termios after SIGCONT, including after the
first mode reacquisition. After suspension, the existing maintenance tick checks
the controlling terminal's native state and repairs raw-mode drift without
trusting the input library's cached flag or relying on a fixed sleep. A late
shell write must not leave single-key input waiting for a newline.

Suspension does not issue a runtime cancellation or change goal policy.
Direct external SIGTSTP, background `bg` resume, and platforms without Unix job
control are outside this keyboard-driven contract.

## Validation Matrix

| Contract | Existing proving surface |
| --- | --- |
| RUN-01 | Busy-submit tests, queued-input tests, queue/approval render tests |
| RUN-02 | `controller::cancellation_tests`, typed query-control races, and `tasks::tests::query_lifecycle` scripted cancel/interrupt/task-return interleavings |
| RUN-03 | Pending-input dispatch, permission-mode tests, approval card render tests |
| RUN-04 | `TuiHarness` lifecycle tests, runtime event projection tests, transcript restore tests |
| RUN-05 | Cleanup failure injection and Unix PTY subprocess tests for normal, error, partial-startup, and panic exits |
| RUN-06 | Production terminal bytes parsed by a terminal emulator: preserved shell history, resize, blank-cell repaint, synchronized frames, and exit cursor; focus event projection |
| RUN-07 | Isolated PTY with a job-control shell: actual stop/foreground resume, shell termios, input-stream restart, repaint after resize, and repeated cycles |

## Open Risks

- The scripted harness proves reducer/render behavior, not OS terminal key
  encoding, clipboard integration, or PTY restoration.
- New reconnect, event-ordering, or approval changes require targeted event
  sequences; existing happy-path coverage is not a universal safety proof.
- Queue-vs-steering semantics need an explicit product decision before new
  shortcuts are exposed. Busy-time command availability follows CMD-03.

## Source Journals

- [TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
- [TUI test harness](../journal/2026-08-02-tui-test-harness.md)
- [Goal resume and permissions](../journal/2026-09-16-goal-resume-permission-tui.md)
- [Incremental Markdown](../journal/2026-10-03-incremental-markdown.md)
- [Turn cancellation barrier](../journal/2026-10-03-turn-cancellation-barrier.md)

- [Terminal restoration](../journal/2026-10-02-tui-terminal-restoration.md)
- [Inline terminal viewport](../journal/2026-10-03-inline-terminal-viewport.md)
- [Interrupt, quit, and Unix job control](../journal/2026-10-03-tui-interrupt-suspend.md)
- [Terminal review follow-up](../journal/2026-10-03-terminal-review-follow-up.md)
