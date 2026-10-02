# Display Text Boundary

## Problem

Untrusted model, tool, restored transcript, and pasted text must not retain
terminal controls. Independent delta sanitization leaks split escape suffixes
and changes split CRLF into two newlines. A line-count-only progress limit does
not bound a single long line.

## Scope

- Presentation-local sanitization before markdown and transcript ingestion.
- Paste sanitization before active-surface routing and burst buffering.
- Stateful per-source tool progress, bounded by bytes and logical lines.
- Canonical visual rows and content-sensitive selection snapshots.
- Grapheme-safe display-column truncation of diagnostic rows, startup labels,
  paths, and session titles.

## Non-Goals

- Changing runtime events, provider payloads, durable formats, or tool artifacts.
- Emulating a terminal, interpreting cursor movement, or retaining ANSI colors.
- Bounding complete assistant responses or the entire conversation history.
- Distinguishing concurrent legacy events that have no invocation identity.

## Architecture

One incremental sanitizer carries escape-parser and CRLF state, not escape
payload bytes. Complete-message and paste sanitization reuse that parser. Each
assistant/thinking stream and each tool invocation/output stream owns separate
state. A new turn or stream cannot inherit an unfinished control sequence.

Transcript constructors sanitize complete display messages; restored entries
use the same constructors. Shared styled-row layout provides a defensive
boundary for derived metadata and older entries without flattening styles.
This boundary does not replace sanitization before markdown parsing.
Bottom-pane activity view construction applies the same physical-line rule to
tool errors, notices, and status metadata before building terminal spans.

Progress entries retain an invocation identity and stdout/stderr identity.
Interleaved calls update their own entry rather than merging based on the last
role label. Sanitized characters enter a rolling tail directly: a huge input
does not first allocate a huge sanitized or formatted display string.

## Contracts

- CSI, OSC, DCS, SOS, PM, APC, generic escape sequences, and nonprinting C0/C1
  controls are removed regardless of chunk boundaries. Partial sequences never
  enter display storage. Unterminated control strings are discarded, not shown.
- CR becomes a newline immediately; an immediately following LF is suppressed
  even when it arrives in a later delta. Ordinary explicit newlines remain.
- Display tabs expand to four spaces. Paste preserves tabs and normalizes CRLF
  before routing; escape/control removal precedes any burst placeholder.
- Every tool-progress message, including its bounded label and truncation
  marker, is at most 16 KiB and 16 logical lines. Eviction preserves UTF-8 and
  the newest visible output. Empty/control-only output creates no visible card
  but still advances its source parser state.
- Structured progress preserves call IDs; direct terminal progress preserves
  terminal IDs. Legacy identity-free events are keyed by name and stream only.
- Tool completion retires its live parser/tail; turn commit, reset, and restore
  retire all live tails. Complete terminal-output previews sanitize before
  splitting lines, retaining at most 16 KiB and six nonempty lines. Typed
  terminal transcript payloads are display projections, not raw tool artifacts.
- Selection uses the latest canonical immutable visual rows. A same-length
  middle edit must refresh copy/highlight even when edges and row counts match.
  Unchanged history does not require a full-content hash on every frame.
- Physical styled rows remove standalone zero-width graphemes that Ratatui
  does not render. Combining marks and joiners inside visible clusters remain
  intact, even across style boundaries; the first contributing span owns the
  cluster style. Rendering, widths, highlight, and copy consume this projection.
  Removing an invisible separator can join visible clusters; the final
  projection is re-segmented so repeated normalization cannot change its layout.
- Diagnostic rows sanitize all metadata before display-column truncation.
  Every row, including headings, prefixes, summaries, and hidden counts, fits
  the requested width. Truncation never splits a grapheme. Session-title middle
  abbreviation likewise uses display columns rather than UTF-8 byte slices.
  Shared startup/path truncation uses the same grapheme-column primitives;
  zero available columns produce empty text, not an overflowing marker.
- The custom terminal diff writer uses that same column policy for glyph
  invalidation, skipped continuation cells, and trailing erase boundaries.
  Legacy escaped cell symbols are measured through the shared sanitizer rather
  than a second ANSI parser; halfwidth sound marks must remain visible cells.

## Validation Matrix

| Boundary | Required evidence |
| --- | --- |
| Stream carry | Every character-boundary split of CSI/string escapes and CRLF; live and finalized markdown agree |
| Source isolation | Interleaved invocations and stdout/stderr; unfinished controls do not affect another source or turn |
| Bounded progress | 10 MB no-newline chunk, many lines, CR progress, multibyte eviction; stored text stays within both limits |
| Paste | Composer, burst expansion, editable overlays, and read-only ownership after sanitization |
| Display coverage | Complete/restored constructors and styled tool-output rows contain no terminal controls |
| Selection identity | Production rows and drag/copy after a same-sized middle replacement |
| Unicode columns | Diagnostic chrome/prefix/message and startup/path/title matrices at zero, narrow, and wide widths; styled cross-span clusters and halfwidth sound marks |
| Visible projection | Standalone zero-width removal, cross-style combining/ZWJ preservation, idempotent re-segmentation, and buffer/highlight/copy agreement |
| Final terminal diff | Halfwidth sound-mark widths agree with Ratatui cells, including escaped legacy symbols; trailing erase starts after the complete cluster |

## Operational Notes

Raw runtime/tool artifacts remain available through their existing owners.
The display tail is intentionally lossy, and explicitly labels truncation.
Parser state has constant size; retained tail storage is bounded. Reading a
large chunk still requires work proportional to its input size.

## Open Risks

- Tests do not prove physical-terminal latency or native clipboard acceptance.
- Legacy progress without a call/terminal ID cannot separate same-name calls.

## Source Journals

- [Display text boundary](../journal/2026-10-03-display-text-boundary.md)
- [Unicode display and editing boundaries](../journal/2026-10-03-unicode-boundaries.md)
