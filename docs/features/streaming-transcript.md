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
- Chunk-independent display sanitization and bounded tool-progress tails; see
  [display text boundary](display-text-boundary.md) for the owning contract.

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

The incremental source cache performs no parsing during delta ingestion.
Presentation access materializes at most once for a changed source revision,
using newline-completed top-level block boundaries to retain a stable prefix.
Incomplete lines remain a replaceable preview, never a stable boundary.
Retained source offsets include indentation that the parser may skip. The
canonical writer resumes with the stable prefix's root formatting state;
separator rows are not reconstructed with independent block-joining rules.
Confirmed tables and following source are held until canonical finalization;
reference definitions appearing in a mutable tail require explicit source-wide
invalidation. After that replay, an owned reference context lets later blocks
resolve prior definitions without parsing retained source. Resolution preserves
the parser's Unicode case folding and first-definition precedence. Source
replacement resets source, references, and row boundaries together.

An unindented plain paragraph may extend the rows produced by its first
canonical parse. Eligibility examines all source in the candidate once, then
only appended characters. Every physical line must start with an ASCII letter
or a non-ASCII character other than whitespace, controls, or a BOM. Markdown
delimiters, escapes, entities, control characters, blank lines, indentation,
and hard-break spaces leave this path. Rejected candidates
are not rescanned until the canonical stable source boundary advances. Pending
trailing spaces remain separate from displayed text until subsequent content
makes them visible. Completed plain lines may be promoted to shared visual
blocks, but structural syntax arriving later advances the replay epoch before
canonical rendering can revise those rows. No fixed-length syntax sample is
used. This bounds eligible paragraph source work and completed-line reuse;
wrapping a single growing physical line remains a separate cost.

Reference reuse is conservative about the parser's expansion budget. The
number of closing brackets multiplied by the largest decoded destination-plus-
title size bounds possible expansion. Check this bound against each parser's
`max(source bytes, 100,000)` budget, for both the whole document and each mutable
suffix or stable-prefix pass. The document count is accumulated from new source
only; fragment checks scan source already subject to parsing. If any bound can
reach its budget, use canonical full-source parsing until reset. This preserves
resource-limit behavior without imposing a fixed document-length cutoff.

Visual-row reuse separates immutable committed blocks from the replaceable
active tail. Viewport counting, visible rendering, and selection share the same
materialized styled rows and plain text. Frame/scroll updates must not clone,
wrap, stringify, or hash unchanged historical rows. Appended committed turns
reuse prior blocks; width, thinking visibility, cwd, replacement, and reset
invalidate the appropriate blocks. Semantic and syntax theme revisions invalidate
committed styles and the stream's materialized Markdown, even without a new
delta. Reinstalling the same resolved theme preserves the caches.

Active-prefix assembly is keyed by session-owned mutation revisions for its
owned inputs, plus width, theme, runtime phase, busy state, execution mode,
thinking visibility, interaction selection, and stream presence. Revisions
change before mutable access, including nested
edits and replacement; retained identity tokens prevent allocator-address reuse
from matching an old cache entry. Reads do not hash or compare accumulated input.
An unchanged key reuses the prefix without assembling or comparing logical rows.
A changed key may rebuild the prefix; complete styled-line equality can still
retain wrapped rows if that rebuild has no visible effect.

An eligible final streaming response is separate from that prefix. The session's
stream state owns source and layout caches with separate borrow lifetimes. The
source collector exposes styled rows, replay epoch, and stable boundaries without
depending on renderer types. Layout reads may coexist with a retained immutable
source-row view. The visual-row cache is keyed by replay epoch, width, and
full/compact view. Append-only source length identifies a revision only within an epoch;
replacement, finalization, source-wide reference invalidation, and open-fence
fallback advance the epoch. Stable logical body rows are promoted to immutable
wrapped blocks, while preview and truncation-summary rows remain replaceable.
An unchanged response read traverses neither source rows nor stable body rows.
Prefix cards retain their ordering, suppression, and thinking visibility.
Response-body appends do not invalidate the prefix key. A visible live-thinking
cell occupies a separate replaceable row block between two retained static
sections. Thinking source and displayed-duration changes refresh that block
without reassembling, comparing, or wrapping those sections. Empty/nonempty
thinking transitions, stream presence, and other structural inputs still
invalidate the surrounding layout. Replacing a runtime snapshot or any other
tracked prefix input also invalidates it. An unchanged phase detail from
successive deltas preserves its revision.

Committed and streaming block indexes use a persistent binary-carry forest.
Its balanced subtrees cache row counts, and its root list has at most
`1 + floor(log2(block_count))` entries for nonempty history. A retained snapshot
can therefore require logarithmic root-handle copies on append, not a copy of
every previous block. Indexed row access is logarithmic in block count. A frame
joins history/prefix and response without building a linked chain per delta or
flattening either row collection.

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

The source cache, frame scheduler, and visual-row cache are separate work
boundaries for issue #921:

- Append bookkeeping examines new source, not the complete accumulated string.
- Completed stable blocks are not re-parsed or copied on each delta. Mutable
  markdown remains replaceable until its structure is stable.
- Render frames reuse unchanged styled/wrapped rows without cloning the whole
  transcript. Selection invalidation tracks all relevant content changes
  without hashing every unchanged historical row on every frame.
- Width, content replacement, finalization, and thread/reset boundaries
  invalidate the appropriate source/layout cache explicitly.
- Response promotion must preserve the canonical stream chrome and complete
  styled rows, including blank rows and the compact view's four-row head and
  remaining-row summary. A width/view/epoch change may cold-rebuild the response;
  unchanged frames and scroll/copy must reuse its retained visual rows.
- Closing fences, table delimiters/rows, list tightness, and references must not
  leave duplicated or stale committed output. Complete-message rendering is
  the final correctness oracle.
- Open-fence fast paths preserve canonical styled rows, including empty code
  lines, for both labelled and unlabelled fences at every chunk boundary.
- Committing raw stream text does not render rows that are immediately
  discarded; the committed-cell renderer owns complete-message presentation.
- Live thinking selects its four-row tail from borrowed materialized Markdown
  rows before copying span contents. Hidden thinking rows must not be traversed
  or cloned by the presentation cell. The heading, duration, hidden-row count,
  dim styling, and finalization order retain their existing behavior. This bound
  applies to row projection; Markdown parsing and wrapping long selected rows
  remain separate costs.
- A confirmed table may start inside the previously mutable paragraph. Holding
  that table must preserve all preceding prose with canonical paragraph spacing,
  regardless of delta boundaries, and release the complete table on finalization.

## Validation Matrix

| Boundary | Required evidence |
| --- | --- |
| Frame coalescing | Synthetic-time draw counts for burst requests and continuous traffic |
| Last update and idle | Independent deadline wake; newest production projection rendered without another event |
| Late frames and deadline stability | No catch-up burst; repeated requests do not postpone the pending deadline |
| Event preservation | Apply all ordered deltas; fewer paints still show the complete final response |
| Markdown work | Parse/source-byte counts over long multiline streams, including mutable structural tails |
| Plain paragraphs | Linear parse/eligibility bytes and rendered rows for word and soft-break deltas; late syntax, whitespace, Unicode splits, replacement, and theme invalidation agree with canonical rows |
| Ingestion and repeated reads | No parsing per delta; no parse or stable-row clone on unchanged presentation reads |
| Row reuse | Rows wrapped, cloned, and hashed per delta/frame; unchanged history remains untouched |
| Layout invalidation | Full middle-row text/style/alignment changes; width/cwd/visibility, theme, append, replacement, reset, and restore; mixed mutation sequences against full rendering |
| Shared history | Retained styled/text allocations across appended turns and indexed windows across block boundaries |
| Active response | Production clone/wrap/text work over long unchanged and growing streams; stable allocation reuse, compact transitions, and preview selection refresh |
| Live thinking | Production cell copy counts depend on the selected four-row tail, not accumulated rows; styled head/tail projection, empty input, and duration remain correct |
| Active prefix | Production assembly counts stay flat on unchanged paints, scrolling, composer-only edits, and response appends; nested same-length changes, replacement, interaction/queue state, thinking source/duration, theme, and width match uncached rendering |
| Thinking slot | Growing thinking and duration ticks retain both static sections; production wrapping touches only the selected window and chrome; empty/visible transitions, spacing, history dividers, and cross-section copy match uncached rendering |
| Table boundary | Tables interrupting mutable paragraphs preserve the canonical preceding prose at every chunk split and finalize to complete-message rows |
| Reference context | Appends after early/late definitions avoid retained-source parsing; Unicode labels, duplicate precedence, late definitions/titles, images, and expansion-budget fallback match canonical styled rows |
| Replay epoch | Same-length replacement; finalization without appended source; fence closer/normalization/highlight-limit and reference replay retain no stale styled rows |
| Persistent index | Thousands of variable-size blocks with retained snapshots; logarithmic roots, balanced subtree order, exact indexing, and joined-boundary copy |
| Correctness | Production renderer and copy/selection agreement after streaming, finalization, resize, and reset |

## Operational Notes

The scheduler is presentation-local. It does not throttle runtime event
delivery or alter provider behavior. Coalescing does not reduce the work of a
single expensive reducer or frame, and does not guarantee terminal throughput
on a slow output device.

## Open Risks

- Historical and eligible streaming-response styled/wrapped rows are shared.
  Unchanged active prefixes bypass assembly and styled-line comparison. Changed
  structural prefix inputs still require full prefix assembly/comparison and
  potentially rewrapping. Live thinking source/duration updates replace only
  their visible block while retaining surrounding rows. The
  assembly counter covers actual assembly calls; row counters do not measure
  source bytes, parser/sanitizer work, or forest metadata. This is not a complete
  per-delta bound.
- Live thinking's body-copy counter covers only the selected four-row window.
  Committed thinking still parses its message on a prefix cache miss;
  live source parsing and wrapping unusually long selected rows are not bounded
  by the window's row count.
- Persistent-index append can copy logarithmically many root handles while an
  older snapshot is retained, and can create logarithmically many carry nodes.
  No constant-cost metadata append or constant-cost indexed access is claimed.
- Deterministic scheduler and renderer tests do not prove OS terminal latency,
  terminal key encoding, viewport lifecycle, or clipboard acceptance.
- Arbitrary markdown may have a long mutable suffix. Work bounds must distinguish
  new source, unstable structure, and one-time full finalization/reflow.
- Unindented top-level open fences reuse syntax state for completed code lines.
  Quoted/indented fences, normalization, closer candidates, newly parsed reference
  definitions, and potential reference-expansion exhaustion conservatively use
  canonical mutable-tail or full-source replay. Ordinary blocks after retained
  definitions reuse the reference context; replay is not permanent merely
  because a document contains references.
  Ineligible long paragraphs/lists still require mutable-tail work; this is not
  an unconditional O(new-delta) guarantee for every Markdown document.
- Source-cache work counters exclude the display sanitizer and control-token
  scrubber. Terminal-control parsing is incremental with constant-size carry;
  ordinary angle brackets use bounded control-token recognition without source
  replay. Completed legacy markers replay once per affected delta; internal
  blocks and DeepSeek evidence retain canonical replay for subsequent nonempty
  deltas. Those fallbacks can still revisit accumulated source. See
  [display text boundary](display-text-boundary.md). Neither boundary proves a
  complete per-delta work bound for arbitrary Markdown.

## Source Journals

- [Frame coalescing](../journal/2026-10-02-tui-frame-coalescing.md)
- [Incremental markdown](../journal/2026-10-03-incremental-markdown.md)
- [Plain paragraph streaming](../journal/2026-10-05-long-mutable-markdown.md)
- [Streaming reference context](../journal/2026-10-04-streaming-reference-context.md)
- [Shared transcript rows](../journal/2026-10-03-transcript-row-reuse.md)
- [Active streaming rows](../journal/2026-10-03-active-stream-rows.md)
- [Bounded thinking window](../journal/2026-10-03-bounded-thinking-window.md)
- [Active prefix cache](../journal/2026-10-03-active-prefix-cache.md)
- [Live thinking row slot](../journal/2026-10-03-live-thinking-row-slot.md)
- [Display text boundary](../journal/2026-10-03-display-text-boundary.md)
