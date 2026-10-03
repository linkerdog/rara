# Display Text Boundary

## Summary

Issue #923 now has presentation-local stream sanitization, bounded per-source
tool progress, safe complete-output previews and paste ingestion, and focused
selection identity coverage. Runtime events, provider payloads, schema,
dependencies, and build configuration are unchanged.

## Background

The frozen parent was `85ed0e6a26aebf60c5e6e2f539026dffcf9eba30` (PR #942).
Five focused baseline checks failed for the intended regressions: stream
storage retained ANSI/CR, paste retained controls, two same-name identified
calls merged, and a no-newline 10 MB chunk produced a 10,485,777-byte entry.
The selection edge-hash cache had already been replaced by canonical immutable
rows; this checkpoint verifies a same-length in-place middle edit rather than
introducing another cache or content hash.

Before implementation, the relevant local reference sources were inspected:

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`:
  `history_cell/messages.rs::sanitize_user_text`, composer paste routing, and
  `ansi-escape/src/lib.rs` parsing/tab expansion. Input sanitation happens
  before storage and terminal styles are not emitted as raw escapes.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
  `hooks/useTextInput.ts`, `components/shell/ShellProgressMessage.tsx`, and
  `OutputLine.tsx`. Input strips ANSI before insertion; progress uses cleaned
  display tails and complete-content memo dependencies. This does not establish
  a chunk-state guarantee in the reference implementation.

## Scope And Key Decisions

- A constant-size parser tracks CSI/string/generic escapes and CRLF across
  deltas. No partial escape payload is retained. Display tabs expand to four
  spaces; paste reuses the parser while preserving tabs.
- Assistant and thinking streams sanitize before markdown/control-token source
  ingestion. Complete transcript constructors also sanitize display text,
  including restored messages and typed terminal metadata.
- A shared sanitized rolling tail evicts whole Unicode scalars during ingestion,
  before formatting. Progress messages include their bounded header/truncation
  marker in the 16 KiB/16-line budget. Complete terminal previews clean before
  line splitting and keep at most 16 KiB/six nonempty lines.
- Structured call IDs were already available through the production event bus;
  they are no longer dropped by progress routing. Direct terminal IDs and
  stdout/stderr keep independent state. Completion and turn/reset/restore
  release live buffers. Legacy identity-free events remain keyed by name/stream.
- Progress payload IDs are presentation-local and are not added to persisted
  entries. Completed turns retain only their existing role/message projection.
- Canonical styled-row layout sanitizes derived/legacy display metadata with
  one parser across spans. Explicit line breaks remain owned by text ingestion;
  Ratatui `Line` is a physical-row boundary. Existing render-site guards remain
  defensive rather than becoming the primary streaming boundary.
- Bottom-pane activity view construction sanitizes tool/error notice metadata
  through the same physical-line boundary. Runtime phase detail is display-safe.
- The unused-code audit traced the existing `TuiEvent::ToolProgress` presentation
  entry to structured runtime consumption. That entry is wired with the
  existing call ID instead of being bypassed or warning-suppressed. The event is
  internal to the private TUI module, not a public runtime protocol change.
- Moving progress out of `runtime/events/helpers.rs` also brings that touched
  source below 1,000 lines; no unrelated module redesign is included.

## Validation

Focused validation includes every character-boundary split of CSI, OSC, DCS,
SOS, PM, APC, generic/C1 escapes and CRLF; huge unterminated control payloads;
bounded byte/line/capacity checks with UTF-8; production event-bus identity;
direct terminal IDs and completion; assistant/thinking finalization; large
paste expansion and overlay routing; terminal metadata rendering; and
same-length middle-row drag/copy refresh through production row materialization.

Final source checks:

| Check | Result |
| --- | --- |
| `cargo test --locked --lib tui:: --quiet` | 801 passed, including 25 new regressions |
| `cargo test --locked --lib --quiet` | 1,666 passed; one existing explicitly paid-call experiment ignored |
| `cargo clippy --locked --all-targets -- -D warnings` | Pass |
| `cargo fmt --check` | Pass |
| `bazel test //:rara_unit_tests --test_output=errors` | 1,666 passed; the same paid-call experiment ignored |
| `git diff --check` | Pass |

The final default Bazel invocation was
`edeccf54-593c-46d7-b421-cb9a04b7d88a`. No snapshots changed. Touched source
files remain below 1,000 lines; `runtime/events/helpers.rs` is now 979 lines.
The existing macOS debug-linker compact-unwind diagnostic remains; no new Rust
compiler or Clippy warning is introduced.

## Follow-Ups

- Exact-head remote CI/review/merge and bounded physical-terminal acceptance
  remain separate delivery gates; no merge or terminal acceptance is claimed.
- Source/layout counters do not include every sanitizer/scrubber/allocation
  cost. Control-token fallback, general active-prefix assembly, and arbitrary
  mutable Markdown remain part of #921, not resolved by this checkpoint.
- A byte cap bounds retained display text, not complete conversation history,
  raw tool artifacts, active invocation count, or input-processing latency.

The canonical contract is [display text boundary](../features/display-text-boundary.md).

## Review Integration Checkpoint

Merged updated parent `179b2d3d52ac2f633c5f7e2978743675c51f9f6f` normally.
The documentation merge preserves both paste ordering and sanitization, and
both terminal restoration and display feedback. Dependency/build inputs are
unchanged relative to that parent.

Two focused RED regressions reproduce the review findings: an unfinished
control string hides the rest of the message, and ESC followed by a newline
consumes the next printable character. The sanitizer now treats CR/LF as a
logical-line recovery boundary in every parser state. CRLF remains one newline
across chunk boundaries, embedded inline controls such as NUL keep the existing
same-line escape behavior, and terminal controls never enter display storage.
The recovery matrix covers OSC, DCS, SOS, PM, APC, their C1 forms, pending ST,
incomplete CSI, and generic escapes at every two-cut character boundary.

This is a transcript policy, not terminal emulation. Multiline terminal-control
payloads are intentionally ended at the first line boundary. Single-line
payloads remain discarded until their terminator, and the 10 MB discard test
still proves constant-size parser state. The complete-message, thinking, and
terminal-preview tests now explicitly expect text after that recovery boundary.
This preserves visible following lines without exposing escape commands or
buffering an unbounded candidate control string.

The reference inspection reconfirmed Codex's shared ANSI/tab display boundary
(`ansi-escape/src/lib.rs`) and Claude Code's cleaned-input and cleaned-tail
paths (`hooks/useTextInput.ts`, `components/shell/ShellProgressMessage.tsx`).
Neither reference establishes this incremental recovery policy; the local
contract and chunk-split regressions do.

Additional review checks cover tool-result ingestion for MCP output, LSP
diagnostics, file reads, and patch diff previews, checking both stored entries
and rendered text. File reads expose their action/path, not the raw body; LSP
JSON keeps escaped data in storage and exercises the display boundary after
field decoding. Another regression uses different tool-call and PTY IDs:
the structured result retires its call buffer, then the typed terminal result
retires its terminal buffer while retaining an unrelated invocation. The
execution layer uses the same tool-call ID for progress and result; no fallback
that merges concurrent same-name calls is needed.

Unicode directional formatting and invisible grapheme components retain their
current behavior. Removing all format characters would damage legitimate
scripts and emoji; a separate annotation/escaping policy remains in the spec
and active TODO. CR progress continues to append normalized logical lines in a
bounded tail rather than emulate carriage-return overwrites. The existing
selection-cache test is integration coverage, not claimed as new RED evidence.

Current TUI validation (`cargo test --offline --locked --lib tui:: -- --nocapture`)
reports 856 passed, one existing ignored test, and no failures. No snapshots
changed. The helper module remains 979 lines and every touched Rust source stays
below 1000 lines. Exact-head CI and physical-terminal acceptance remain separate.
