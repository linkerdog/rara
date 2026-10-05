# Terminal Feedback

## Problem

Long turns need visible window identity and an optional signal when attention
is required away from the focused terminal.

## Scope

The interactive terminal owns window titles, focus-gated bells and OSC 9
notifications. Configuration lives under `tui.terminal`:

```json
{"tui":{"terminal":{"title":true,"notifications":"off"}}}
```

`notifications` accepts `off` (default), `bell`, or `osc9`. Titles default to
enabled, except on `TERM=dumb`. Configuration takes effect on the next TUI
launch. Headless, ACP and Wire clients do not emit terminal feedback.

## Non-Goals

Desktop notification services, title customization menus, notification previews
containing conversation content, and querying the terminal's existing title are
outside this contract.

## Architecture

Semantic notification state belongs to the TUI projection. Accepted approval
events and completed query tasks enqueue feedback; rendering and snapshot replay
do not manufacture completion events. The event loop is the only live output
writer. Terminal lifecycle ownership includes title stack restoration.

## Contracts

- The title contains an ASCII activity label, workspace basename and named
  thread (or a short thread ID). Pending decisions take priority over running
  work; otherwise the label is `idle`.
- Repeated frames with the same title produce no title writes. Resume after
  suspend invalidates this cache. Rename, new thread and restore update the
  displayed identity without synchronous storage reads.
- Title payloads pass through the shared escape parser, remove control and
  bidi formatting characters, normalize whitespace and cap at 240 Unicode
  scalars. Notifications contain fixed messages, never command/model content.
- Save the window title with `CSI 22;2 t`, set it with OSC 2, and restore with
  `CSI 23;2 t`. These are xterm-compatible window-title protocols. Each accepted
  save owns exactly one restoration, including setup errors, unwinding, caught
  owner panics and suspend. A failed partial save does not pop a parent's stack;
  a failed flush after an accepted save still restores. Other mode cleanup
  proceeds after any title output failure.
- Multiplexer output uses the same tmux/screen passthrough framing as OSC 52;
  BEL stays unwrapped so the multiplexer can handle its bell policy.
- Notifications are opt-in and consumed only for newly accepted events while
  unfocused. Focusing again before emission discards the pending notification.
  Focus changes never resurrect an old event. Approval has priority over a
  completion in the same output cycle and repeated approval IDs are deduplicated.
- Successful query completion signals only after the task and ordered runtime
  stream have finished. Automatic continuations and queued follow-ups suppress
  intermediate completion signals. Pending approvals signal attention instead.
  Query failures can signal failure; user cancellation and maintenance commands
  do not signal completion. Restoring a saved pending decision is silent.

## Validation Matrix

| Boundary | Evidence |
| --- | --- |
| Config | Defaults, legacy config, each method, invalid values, round trip |
| Output | Exact title/BEL/OSC 9 and passthrough bytes, vt100 screen unchanged |
| State | Focus, repeated approvals, completion barrier, continuation, restore |
| Lifecycle | PTY normal/error/panic/setup failure, repeated acquisition, suspend |
| Integration | Rename/new/restore identity and production event-loop output |

## Operational Notes And Open Risks

Terminals must support the xterm title stack to restore a prior title. Users of
incompatible terminals can set `title` to false. No portable query establishes
this capability without consuming terminal input. OSC 9 delivery and multiplexer
passthrough depend on terminal settings; successful writes do not guarantee an
OS notification. Focus starts as focused, suppressing notifications until an
actual focus-loss report arrives.

## Source Journals

- [Terminal feedback implementation](../journal/2026-10-05-terminal-feedback.md)
