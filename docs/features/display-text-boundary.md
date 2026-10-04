# Display Text Boundary

## Problem

Untrusted model, tool, restored transcript, and pasted text must not retain
terminal controls. Independent delta sanitization leaks split escape suffixes
and changes split CRLF into two newlines. A line-count-only progress limit does
not bound a single long line.

## Scope

- Presentation-local sanitization before markdown and transcript ingestion.
- Chunk-independent internal-control cleanup in assistant Markdown streams.
- Paste sanitization before active-surface routing and burst buffering.
- Stateful per-source tool progress, bounded by bytes and logical lines.
- Canonical visual rows and content-sensitive selection snapshots.
- Grapheme-safe display-column truncation of diagnostic rows, startup labels,
  paths, and session titles.
- Visible annotations for bidirectional controls, with source-offset-aware
  editing and unchanged submitted input.

## Non-Goals

- Changing runtime events, provider payloads, durable formats, or tool artifacts.
- Emulating a terminal, interpreting cursor movement, or retaining ANSI colors.
- Bounding complete assistant responses or the entire conversation history.
- Distinguishing concurrent legacy events that have no invocation identity.
- A general Unicode spoofing detector, normalization, or annotations for every
  invisible character. Physical rows follow the zero-width projection contract
  below; visible clusters retain their joiners and variation selectors.

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

- After each assistant delta, Markdown input equals complete-message internal
  control cleanup of the terminal-sanitized source prefix. Split legacy markers,
  internal-block separators, late DeepSeek evidence, and orphaned DSML tails
  cannot depend on provider chunk boundaries. Literal `<think>` remains visible
  without DeepSeek control evidence.
- Ordinary angle brackets, including arbitrarily long partial marker names,
  use bounded recognition state without revisiting accumulated source. Completed
  legacy markers can revise visible text through canonical cleanup. Internal
  block or DeepSeek evidence switches that stream to canonical replay, including
  later plain chunks: these rules can retrospectively change earlier text and
  are not covered by an O(new-delta) ingestion claim.
- CSI, OSC, DCS, SOS, PM, APC, generic escape sequences, and nonprinting C0/C1
  controls are removed regardless of chunk boundaries. Partial sequences never
  enter display storage. A CR or LF ends any unfinished control sequence and
  preserves that logical line boundary; later lines resume ordinary parsing.
  This intentionally does not emulate multiline terminal control payloads.
  Before a terminator or line boundary, string payload remains discarded with
  constant-size state, including arbitrarily long single-line payloads.
- CR becomes a newline immediately; an immediately following LF is suppressed
  even when it arrives in a later delta. Ordinary explicit newlines remain.
- Display tabs expand to four spaces. Paste preserves tabs and normalizes CRLF
  before routing; escape/control removal precedes any burst placeholder.
- The twelve Unicode `Bidi_Control` characters (`U+061C`, `U+200E..U+200F`,
  `U+202A..U+202E`, and `U+2066..U+2069`) become visible code-point labels such
  as `⟦U+202E⟧` in display text, before Markdown or width calculation. Labels
  have no Markdown delimiter semantics and repeated sanitization is stable.
  Escape payloads remain discarded rather than exposing labels from them.
  This set follows [Unicode 17.0 PropList](https://www.unicode.org/Public/17.0.0/ucd/PropList.txt).
- Paste, draft/history editing, submission, runtime events, and tool artifacts
  retain these Unicode characters. Editors project the same visible labels
  while mapping cursor positions and navigation to original character offsets;
  a label is one source character and deletion removes that character. Transcript
  selection copies the displayed label, not a hidden directional instruction.
  Masked credential editors continue to show one mask per source character.
- Other format characters are not blanket-filtered or normalized. Visible
  clusters retain ZWJ/ZWNJ, variation selectors, combining marks, and emoji tags.
  Standalone zero-width clusters still follow the physical-row rule below.
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
| Internal control carry | Every character-boundary split and character-sized deltas agree with canonical source cleanup and Markdown rows; ordinary angle brackets and long unfinished legacy names do not replay accumulated source |
| Source isolation | Interleaved invocations and stdout/stderr; unfinished controls do not affect another source or turn |
| Bounded progress | 10 MB no-newline chunk, many lines, CR progress, multibyte eviction; stored text stays within both limits |
| Paste | Composer, burst expansion, editable overlays, and read-only ownership after sanitization |
| Display coverage | Complete/restored constructors and styled tool-output rows contain no terminal controls |
| Selection identity | Production rows and drag/copy after a same-sized middle replacement |
| Unicode columns | Diagnostic chrome/prefix/message and startup/path/title matrices at zero, narrow, and wide widths; styled cross-span clusters and halfwidth sound marks |
| Visible projection | Standalone zero-width removal, cross-style combining/ZWJ preservation, idempotent re-segmentation, and buffer/highlight/copy agreement |
| Bidi annotations | All twelve controls across chunk boundaries, styled rows, Markdown, bounded progress, and selection; paste/submission preserve source, editor cursor/wrapping/deletion use source offsets; legitimate joining and emoji survive |
| Final terminal diff | Halfwidth sound-mark widths agree with Ratatui cells, including escaped legacy symbols; trailing erase starts after the complete cluster |

## Operational Notes

Raw runtime/tool artifacts remain available through their existing owners.
The display tail is intentionally lossy, and explicitly labels truncation.
Parser state has constant size; retained tail storage is bounded. Reading a
large chunk still requires work proportional to its input size.

## Open Risks

- Tests do not prove physical-terminal latency or native clipboard acceptance.
- Legacy progress without a call/terminal ID cannot separate same-name calls.
- Bidi labels expose explicit direction controls, not all visually confusable
  text or invisible payloads. A literal label can look identical to an annotation;
  source inspection belongs to raw runtime/tool artifacts, not transcript copy.
  Natural RTL text still depends on terminal shaping and ordering support.
- Streams containing internal blocks or DeepSeek evidence replay canonical
  control cleanup on each later nonempty delta. A future incremental replacement
  must preserve transformation order, delayed separators, retrospective leading
  think removal, malformed DSML literals, and persistent orphan-tail suppression.

## Source Journals

- [Display text boundary](../journal/2026-10-03-display-text-boundary.md)
- [Unicode display and editing boundaries](../journal/2026-10-03-unicode-boundaries.md)
- [Streaming control-token cleanup](../journal/2026-10-04-streaming-control-cleanup.md)
- [Bidirectional control annotations](../journal/2026-10-04-bidi-display-annotations.md)
