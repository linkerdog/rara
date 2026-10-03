# Streaming Transcript

## Problem

Streaming must not repeatedly parse and copy the complete answer or redraw the
complete transcript for every delta. Terminal updates must remain ordered and
the last update must become visible even when no later event arrives.

## Scope

- Coalesced session-local frame scheduling for presentation changes.
- Incremental markdown source, with a stable prefix and replaceable tail.
- Reusable styled visual rows and selection text for unchanged history.
- Deterministic work-count regressions before timing benchmarks.

## Non-Goals

- Changes to runtime event delivery, task completion, or cancellation policy.
- Process-global UI handles or a new runtime protocol.
- Terminal scrollback migration or semantic anchors across reflow.
- A wall-clock latency guarantee while reducers or renderers block the UI task.

## Architecture

Runtime and terminal events continue to update presentation state immediately.
A local frame scheduler remembers one pending deadline, coalescing repaint
requests rather than batching or dropping the events themselves. The event
loop can await this deadline independently of runtime, input, and maintenance
activity. Viewport measurement and painting happen only when a frame is due.

Markdown and visual-row caches are separate boundaries: rate limiting alone
does not make source parsing or each painted frame incremental. The target is
append-only source bookkeeping, stable blocks that are not re-parsed, and a
mutable suffix that may be replaced when later markdown changes its meaning.
Tables and other structurally unresolved blocks must not freeze stale rows.
Finalization must agree with the canonical complete-message renderer.

## Contracts

### Frame Scheduling

- The first dirty frame is eligible immediately.
- Subsequent paints are separated by at least 16,666,667 nanoseconds, measured
  from the actual completion of the previous paint, not an old request time.
- All requests before the next eligible frame share one deadline. More events
  cannot postpone an already pending frame indefinitely.
- Events are applied without waiting for that deadline. A due frame reads the
  latest projection, including the final delta or terminal runtime event.
- A pending frame wakes independently of the existing 166 ms maintenance tick;
  it does not require another key, delta, or completion event.
- No dirty request means no frame timer or repaint. A successful paint retires
  the pending deadline; a failed paint retains the error path, not a hidden retry.
- Terminal input, resize, focus, and runtime feedback use the same frame gate.
  Input/projection changes are immediate; resize requests are retained and the
  frame measures current terminal dimensions. Painting may wait one interval.
- A due composer paste is flushed through the composer edit boundary before
  frame admission, preserving history/cursor bookkeeping and requesting a paint.
- Exiting the session may discard a pending repaint. Terminal restoration does
  not depend on drawing that final frame.

### Incremental Markdown And Rows

These contracts are the remaining implementation target for issue #921, not
claims established by the frame-scheduling checkpoint alone:

- Append bookkeeping examines new source, not the complete accumulated string.
- Completed stable blocks are not re-parsed or copied on each delta. Mutable
  markdown remains replaceable until its structure is stable.
- Render frames reuse unchanged styled/wrapped rows without cloning the whole
  transcript. Selection invalidation tracks all relevant content changes
  without hashing every unchanged historical row on every frame.
- Width, content replacement, finalization, and thread/reset boundaries
  invalidate the appropriate source/layout cache explicitly.
- Closing fences, table delimiters/rows, list tightness, and references must not
  leave duplicated or stale committed output. Complete-message rendering is
  the final correctness oracle.

## Validation Matrix

| Boundary | Required evidence |
| --- | --- |
| Frame coalescing | Synthetic-time draw counts for burst requests and continuous traffic |
| Last update and idle | Independent deadline wake; newest production projection rendered without another event |
| Late frames and deadline stability | No catch-up burst; repeated requests do not postpone the pending deadline |
| Event preservation | Apply all ordered deltas; fewer paints still show the complete final response |
| Markdown work | Parse/source-byte counts over long multiline streams, including mutable structural tails |
| Row reuse | Rows wrapped, cloned, and hashed per delta/frame; unchanged history remains untouched |
| Correctness | Production renderer and copy/selection agreement after streaming, finalization, resize, and reset |

## Operational Notes

The scheduler is presentation-local. It does not throttle runtime event
delivery or alter provider behavior. Coalescing does not reduce the work of a
single expensive reducer or frame, and does not guarantee terminal throughput
on a slow output device.

## Open Risks

- Incremental markdown and visual-row/selection caching remain open after the
  first scheduling checkpoint.
- Deterministic scheduler and renderer tests do not prove OS terminal latency,
  terminal key encoding, viewport lifecycle, or clipboard acceptance.
- Arbitrary markdown may have a long mutable suffix. Work bounds must distinguish
  new source, unstable structure, and one-time full finalization/reflow.

## Source Journals

- [Frame coalescing](../journal/2026-10-02-tui-frame-coalescing.md)
