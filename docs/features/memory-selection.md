# Memory Selection

## Problem

RARA needs one runtime contract for deciding which memory-like material is
available to the model on a turn.

Today the system already has several memory-adjacent inputs:

- workspace memory files loaded through prompt sources;
- compacted thread carry-over;
- active thread state such as plans, pending interactions, latest user request,
  and recent tool results;
- retrieval tool results;
- placeholder thread-history and vector-memory candidates.

Without a first-class selection boundary, prompt assembly can accidentally make
ad hoc inclusion decisions, `/context` cannot explain why something appeared or
was omitted, and future vector or graph retrieval can bypass the same budget and
dedupe rules.

## Scope

`MemorySelection` is the authoritative per-turn view of memory-like material.
It records:

- fixed items that are already selected for the current working set;
- discretionary candidates that may compete for the memory-selection budget;
- available but not injected candidates;
- dropped candidates that lost ranking, dedupe, or budget checks;
- human-readable reasons for selection, availability, and drops;
- budget impact where it can be estimated.

The contract applies before final model-specific prompt rendering and after
runtime sources for the turn are known.

## Non-Goals

`MemorySelection` is not:

- the full context assembler;
- the durable memory store;
- a replacement for `ThreadStore`;
- the compaction algorithm;
- a vector or graph retrieval backend;
- the cross-source retrieval orchestrator;
- a transcript renderer.

It may reference outputs from these systems, but it does not own their storage
or lifecycle.

## Architecture

### Overview

The runtime should make memory selection debuggable in the same spirit as
Gemini CLI's memory surfaces:

- the user can inspect the active context, not only trust that it was assembled;
- source hierarchy and order are visible;
- omitted inputs have explicit reasons;
- the debug view is a runtime artifact, not prose reconstructed by the model.

`/context` is the main inspection command for that artifact.

### Context Selection vs Memory Selection

Context selection is the broader process of building the complete model working
set for a turn. It includes stable instructions, prompt sources, workspace
context, active thread state, selected memory, compacted history, tool
availability, and backend framing.

`MemorySelection` is a narrower sub-step inside that process. It only explains
memory-like sources and recall candidates.

The relationship is:

1. runtime state and prompt sources are collected;
2. retrieval orchestration normalizes source hits into ranked candidates;
3. `MemorySelection` classifies memory-like inputs into selected, available, and
   dropped groups;
4. `ContextAssembler` uses selected memory items together with non-memory
   context layers;
5. backend-specific rendering serializes the final context for the model.

This separation keeps instruction ordering and prompt-source precedence stable
while still making recall decisions inspectable.

### User-Facing Model

Users should be able to read `/context` as a compact tree:

```text
Context
 L instructions
 L workspace
 L thread
 L memory selection
    L selected
    L available
    L dropped
 L compaction
```

The tree should preserve the real assembly order and show file paths or source
labels where they exist. This mirrors Gemini CLI's preference for inspectable
memory lists and import trees instead of hidden concatenation.

### Selected Items

Selected items are memory-like inputs that are part of the current turn working
set.

They include two categories.

Fixed selected items:

- workspace memory that is already an active prompt source;
- compacted carry-over selected by the compaction lifecycle;
- active thread working-set state that must survive restore, such as plans,
  pending interactions, latest user request, and recent tool results.

Discretionary selected items:

- retrieved workspace memory;
- retrieved thread context;
- future vector or graph retrieval candidates that win ranking and budget checks.

Fixed selected items do not compete for the discretionary retrieval budget. They
are listed in `MemorySelection` so `/context` can explain them, not so retrieval
can re-rank them after ownership has already selected them.

Fixed selected items also remain owned by their original assembly layers.
Workspace memory stays under prompt sources, compacted carry-over stays under
compacted history, and active thread state stays under active turn state.
`ContextAssemblyView` should only place newly selected discretionary retrieval
items in `active_memory_inputs`.

### Available Items

Available items are candidates that can be considered or recalled, but are not
injected in the current turn.

Examples:

- workspace memory exists but is not active as a prompt source;
- raw thread history exists but compacted history or focused retrieved context
  already covers the thread need;
- vector memory is configured but ranked retrieval is not implemented yet.

Available items are not failures. They are omitted because another source owns a
more focused view or because the runtime has not activated that source for this
turn.

### Dropped Items

Dropped items are candidates that were considered and lost selection.

Common drop reasons:

- the candidate would exceed the remaining memory-selection budget;
- a higher-priority candidate already covered the same recall need;
- the candidate failed ranking or dedupe rules.

Dropped items should preserve enough detail for `/context` to answer why the
candidate was not injected.

## Contracts

### Runtime View

`MemorySelectionContextView` must remain a stable inspectable structure:

- `selection_budget_tokens`
- `selected_items`
- `available_items`
- `dropped_items`

Each `MemorySelectionItemContextEntry` must carry:

- `order`
- `kind`
- `label`
- `detail`
- `selection_reason`
- `budget_impact_tokens`
- `dropped_reason`

The field names are part of the `/context` and restore/debug contract. They
should not be replaced by opaque prompt text.

### Display Contract

The `/context` display should be dense and predictable:

- show sections in runtime assembly order;
- render source paths as paths, not prose;
- keep selected, available, and dropped memory groups visually separate;
- show cache status only when it is known;
- prefer short reasons over paragraph explanations;
- keep placeholder sources visibly marked as unavailable or not implemented.

### Ordering

Selected item order should follow the assembled turn order as closely as
possible:

1. fixed workspace and compaction inputs;
2. active thread working-set inputs;
3. selected retrieval candidates by priority and ranking.

Available and dropped items should use deterministic order so `/context` output,
tests, and future status surfaces remain stable.

### Budget

The memory-selection budget only governs discretionary memory candidates.

Fixed selected items may still report `budget_impact_tokens`, but they must not
consume the remaining discretionary retrieval budget inside `MemorySelection`.
Overall model-window budgeting stays owned by the broader context assembly and
compaction layers.

Retrieved memory candidates should charge budget against the same rendered
context block that is injected into the next model turn. The shared
header/footer/instruction overhead is charged once per selected retrieved-memory
block, and each later retrieved item only charges its incremental rendered
cost. Multi-line labels and details are normalized before rendering so one
memory candidate cannot break the injected list shape.

### Dedupe

The selector should avoid injecting redundant views of the same source.

Initial rules:

- compacted thread history beats raw thread history;
- focused retrieved thread context beats raw thread history;
- selected workspace memory beats an available-only workspace memory marker;
- future vector and graph retrieval should dedupe against active prompt sources,
  compacted history, and recent tool results.

### Source Ownership

`MemorySelection` should not write memory records or mutate thread history.

Ownership boundaries:

- `WorkspaceMemory` owns prompt-file discovery and cache behavior;
- `ThreadStore` owns durable thread metadata and persisted transcript items;
- compaction owns summary generation and compaction boundary metadata;
- retrieval backends own candidate discovery;
- `MemorySelection` owns per-turn classification, ordering, reasons, and budget
  outcome.

Direct retrieval candidates are injected only after selection. RARA follows the
same reference-agent shape here: stable base instructions remain fixed, while
turn-specific recall is attached as separate contextual material. Selected
retrieval candidates must therefore be rendered as a per-turn internal context
block before the current user request content, not appended to the base system
prompt or persisted as conversation history. The block must not create an extra
user-role message before the request because some providers require strict role
alternation. Retrieval runs once after the user request is accepted, then the
selected candidates are reused for tool-followup model turns until the next
user request.

### Reference Implementation Alignment

RARA should keep the following alignment points with mature coding-agent memory
systems:

- Stable base instructions and prompt-file sources should remain ordered and
  byte-stable. Per-turn recall must be injected as contextual material after
  selection instead of being appended to the base system prompt.
- Recall should run once for a user turn, then be reused for tool-followup model
  turns. Re-running retrieval inside every model-loop iteration wastes
  embedding/search work and can create inconsistent context inside one user
  request.
- Retrieved memory must not become ordinary conversation history. It is an input
  artifact for the current model request and should be regenerated or reused by
  the runtime contract, not checkpointed as if the user had said it.
- The injection shape must preserve provider role constraints. If the transport
  requires strict user/assistant alternation, contextual recall should be merged
  into the current user request content or represented as a provider-safe
  attachment-like input, not inserted as an extra adjacent user message.
- Recall output should be inspectable as structured state. The runtime should be
  able to explain what was selected, what was available, what was dropped, why it
  happened, and how much budget was consumed.

The main architectural difference is storage and selection. File-memory
reference agents commonly keep durable memories as markdown files with a compact
index, scan file headers, then use a side-query selector to choose a small number
of relevant files. RARA instead routes durable workspace memory and thread
context through retrieval backends first, then lets `MemorySelection` make the
final per-turn decision. This keeps RARA compatible with LanceDB-backed full-text,
vector, and later graph retrieval while preserving the same cache-safe injection
boundary.

Future work can borrow three additional ideas without changing this PR's core
contract:

- add a lightweight selector/reranker over retrieval candidates using structured
  output, so the model can choose from a manifest-like view before injection;
- add cumulative surfaced-memory accounting and per-item truncation limits so a
  long session cannot repeatedly surface large recall blocks;
- add a unified attachment-like context carrier for retrieved memory, skills,
  queued input, hook output, and post-compaction restoration. Until that carrier
  exists, prepending an internal context block to the latest user text request is
  the provider-safe bridge.

### `/context` Priority

`/context` is the primary user-facing debugger for this contract.

The `/context` display should have higher implementation priority than broad
retrieval expansion because every later memory feature needs an inspectable
answer to:

- what was injected;
- where it came from;
- why it was selected;
- what was available but omitted;
- what was dropped by ranking, dedupe, or budget;
- how much budget each item consumed;
- whether cache-backed sources were hit or refreshed.

## Validation Matrix

- Unit tests for `MemorySelection` should cover selected, available, and dropped
  paths with deterministic order.
- Context assembler tests should assert that selected memory items appear in the
  assembly view without serializing unavailable or dropped candidates as prompt
  text.
- `/context` tests should verify that selected, available, dropped, and budget
  fields are visible in the command output.
- Restore tests should verify that active thread state remains selected after
  session reload when the underlying runtime state is present.
- Future vector retrieval tests should verify budget exhaustion and dedupe
  reasons before adding broader ranking behavior.

## Open Risks

- Token estimates are approximate and may drift from backend-specific tokenizers.
- Fixed selected items can still overfill the full context window if broader
  context budgeting does not compact early enough.
- Placeholder vector and thread-history candidates can look more complete than
  they are unless `/context` labels readiness and implementation status clearly.
- The boundary between active thread working set and durable thread memory needs
  continued alignment with `ThreadStore`.

## Source Journals

- `docs/journal/2026-04-25-context-assembly-stage1.md`
- `docs/journal/2026-05-01-memory-selection-spec.md`
- `docs/features/retrieval-orchestration.md`
