# Typed TUI Transcript Presentation

## Problem

String labels currently serve as both transcript roles and event dispatch keys.
A spelling error can select a generic transcript fallback instead of the intended
runtime behavior. Re-parsing labels in renderers also obscures which roles are
handled. Textual source inclusion hides module boundaries and effective size.

## Scope

- Internal TUI transcript roles, rendering dispatch, and presentation events.
- Conversion between typed presentation state and existing persisted labels.
- Responsibility-based source/test modules and exhaustive semantic dispatch.
- The ordering domain consumed by the in-process TUI runtime port.

## Non-Goals

This contract does not change runtime or provider protocol roles, serialized
transcript formats, database schemas, public embedding APIs, visible transcript
labels, or rendering order. It does not introduce a new runtime event stream or
infer server restarts from a low sequence number.

## Architecture

Runtime semantics arrive as `RuntimeControlEvent` and are matched by typed
variants. OAuth and model-download progress use dedicated presentation events;
they cannot impersonate assistant deltas or tool results by choosing a label.
The transcript's `MessageRole` owns internal role identity. Display/persistence
labels are derived from it, not parsed repeatedly inside renderers.

Stored rows remain string-based for compatibility. Restoration is the explicit
conversion boundary: known labels become typed roles and unknown historical
labels remain sanitized legacy display labels. Live transcript constructors
require typed roles. Unknown legacy content never becomes a runtime command,
streaming delta, or tool lifecycle event.

## Contracts

### TRANSCRIPT-01: Typed Live Roles And Events

Live transcript construction and semantic rendering use typed roles and
exhaustive matches. There is no implicit string-to-role conversion on the live
path and no arbitrary role-keyed event dispatch fallback. Runtime assistant/tool
behavior remains owned by the existing structured reducer. Delegated tool results
read their summary and pending question from the tool's JSON fields, independently
of the displayed label. Both the summary and question survive in one result.
Shell `rg` actions use the structured command argument for exploration grouping;
non-search shell commands retain running activity. Failed or unrelated tool
results cannot synthesize delegated questions.

### TRANSCRIPT-02: Stable Storage And Display

Known role spellings round-trip exactly through existing persisted rows.
Unknown historical labels retain their sanitized display text and neutral
rendering behavior. Secret redaction, display sanitation, typed payloads, and
user/assistant segment boundaries retain their current behavior.

### TRANSCRIPT-03: Real Module Boundaries

Source and test files stay below 1000 lines and split by responsibility. `mod.rs`
files are facades. Source `include!` is not a substitute for module boundaries.
Moving tests preserves their assertions and snapshot identities/content.

### TRANSCRIPT-04: Sequence Domains Remain Explicit

Deduplication applies to one subscribed runtime ordering domain. Reconnect alone
does not authorize old events or reset a live session/turn fence. A replacement
bus or future transport must establish its ordering identity at the adapter
boundary; a lower sequence by itself is insufficient evidence of a new domain.

## Validation Matrix

| Contract | Evidence |
| --- | --- |
| Typed construction and exhaustive dispatch | Rust typecheck/Clippy; an invalid live role cannot compile |
| Known and legacy role persistence | Round-trip, sanitation, and unknown-label restoration tests |
| Runtime semantics | Existing structured assistant/tool tests and maintenance progress routing tests |
| Visible behavior | Existing cell/render tests and unchanged reviewed snapshots |
| Structural boundaries | Source-size/module audit and preserved test-function inventory |
| Sequence domain | In-process bus/port identity inspection and sequence/reconnect regression evidence |

## Operational Notes

Only the restoration boundary accepts legacy labels. Keep this compatibility
path separate from live construction so it cannot hide misspelled producer
roles. User interaction requirements remain in the interaction specifications.

## Open Risks

Future remote adapters must define reconnect/replay epochs before reusing a
controller across a replacement ordering domain. This internal typing change
must not silently redesign that protocol.

## Source Journals

- [Typed transcript and source boundaries](../journal/2026-10-03-tui-typed-transcript.md)
