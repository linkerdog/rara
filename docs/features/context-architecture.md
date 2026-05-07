# Context Architecture

## Summary

RARA should treat context as a first-class runtime system rather than a
best-effort prompt assembly step.

The target architecture should align more closely with Codex- and Claude-style
agent runtimes:

- stable instructions are layered and persisted separately from transient turn
  state;
- recent active context stays explicit and lossless;
- older thread history is compacted into structured summaries rather than
  dropped or replayed verbatim forever;
- context budgeting is model-aware instead of using one fixed heuristic for all
  backends;
- session/thread state is persisted as structured rollout items, not only as
  flattened chat text.

This document defines the target object model, persistence boundary, context
assembly contract, and staged rollout plan.

## Goals

- Make context assembly deterministic and explainable.
- Separate stable instructions, workspace memory, active turn state, and
  compacted history into distinct runtime objects.
- Persist threads as structured event history that can be resumed, compacted,
  archived, and inspected.
- Make context budgeting model-aware across Codex, hosted providers, and local
  models.
- Turn compaction into an explicit runtime lifecycle event instead of an
  invisible backend detail.

## Non-Goals

- Implementing remote thread storage in the first pass.
- Replacing all existing prompt-building code in one change.
- Designing a new public protocol separate from the current agent/tool loop.
- Solving retrieval quality and vector memory in the same milestone.

## Problem Statement

Today RARA's context behavior is split across:

- prompt-source discovery;
- agent history and token accounting;
- TUI/runtime transient state;
- session persistence;
- backend-specific summarization behavior.

This causes several issues:

- no single object model describes "what counts as context";
- prompt assembly is harder to reason about than it should be;
- resume and compaction operate on partial views of the conversation;
- model-specific context limits are only partially reflected in runtime policy;
- long-lived sessions risk drift because older information is either replayed
  too literally or lost too implicitly.

## Design Principles

1. Context is runtime state, not just prompt text.
2. Stable and transient context must be stored separately.
3. Recent context should stay lossless for as long as budget allows.
4. Old history should be compacted into structured summaries, not silently
   discarded.
5. Context budgeting must be derived from model metadata whenever possible.
6. Persistence should store structured rollout items instead of flattened chat
   strings.

## Context Layers

RARA should assemble model input from five distinct layers.

### 1. Stable Instructions

Stable instructions are long-lived constraints that should be injected on every
turn unless explicitly disabled.

Examples:

- base runtime/system instructions;
- user instruction files such as `~/.rara/AGENTS.md`;
- project instruction files such as `AGENTS.md`;
- runtime policy fields such as plan mode or bash approval mode.

Properties:

- low churn;
- deterministic precedence;
- deterministic ordering: user-level instruction sources come first, followed
  by project instruction sources from workspace root toward the current focus
  directory, so moving into a deeper directory preserves the existing prefix;
- persisted separately from turn history;
- never compacted into conversational summaries.

### 2. Workspace Context

Workspace context is durable but not necessarily injected verbatim each turn.

Examples:

- workspace memory;
- durable repo facts;
- future retrieval-backed memory snippets;
- stored notes derived from previous sessions.

Properties:

- durable across turns and sessions;
- may be selectively injected based on relevance;
- separate from chat history and separate from stable instructions.

### 3. Active Turn Context

Active turn context is the current working set for the agent.

Examples:

- latest user request;
- current plan and plan-step state;
- pending approvals/questions;
- latest tool results;
- most recent assistant/tool transcript items.

Properties:

- always lossless;
- highest priority after stable instructions;
- should be represented as structured turn items, not reconstructed from raw
  transcript text.

### 4. Compacted Thread History

Compacted thread history represents older conversation state that is still
important but no longer worth replaying verbatim.

Examples:

- prior implementation decisions;
- important earlier debugging findings;
- previous user constraints that still matter;
- compacted summaries of older tool/result sequences.

Properties:

- derived from structured history;
- injected as explicit summary items;
- replaces older raw turns after compaction;
- must remain attributable to a compaction event.

### 5. Model-Aware Context Budget

Context assembly must respect model-specific limits.

Examples:

- total context window;
- reserved output budget;
- compaction threshold;
- reasoning-specific overhead where relevant.

Properties:

- backend/model dependent;
- computed before building the final prompt;
- determines when compaction is required.

## Context and Memory Integration

Context and memory should not be treated as the same thing.

### Core Principle

- `context` is the current working set sent to the model for this turn;
- `memory` is a durable knowledge store that may contribute selected items into
  context when relevant.

In other words:

- memory is a source;
- context is an assembled view.

### Stage 1.5 Memory Selection

Before full vector-backed retrieval lands, RARA should still treat memory
selection as an explicit runtime object instead of letting the assembler make
ad hoc inclusion decisions inline.

The canonical contract is `docs/features/memory-selection.md`. This section
keeps the high-level architecture relationship only.

The first cut of `MemorySelection` should:

- collect the currently selected memory-like inputs;
- collect available-but-not-injected inputs separately from dropped ones;
- collect considered-but-dropped inputs;
- preserve human-readable selection and drop reasons;
- expose a bounded selection budget for the current turn.

The next step on top of that first cut is to split the selection surface into:

- fixed memory inputs that are already injected by ownership elsewhere
  (for example workspace memory, compacted carry-over, or active thread
  working-set items such as plan state, pending interactions, and recent tool
  results);
- discretionary retrieval candidates that still compete for the current
  selection budget.

That keeps already-injected context from being re-ranked after the fact while
still letting `/context` explain:

- which retrieval candidates were considered;
- which candidates won the current budget;
- which candidates were dropped because a more focused source already covered
  the same need;
- which candidates were dropped because the selection budget was exhausted.

The assembler should preserve that ownership split. Fixed items may appear in
the `MemorySelection` explanation, but they should not be re-emitted under the
`active_memory_inputs` assembly layer. That layer is reserved for discretionary
retrieval candidates that actually won selection for this turn.

This lets `/context` explain:

- what won the current memory budget;
- what remained available but not injected;
- which parts of the thread/workspace surface are still placeholder or
  readiness-only paths.

`/context` should be treated as a high-priority delivery surface for this
contract, because memory selection cannot be safely expanded without a clear
debug view of selected, available, dropped, and budgeted sources.

RARA should therefore avoid:

- replaying the full memory store as prompt text;
- treating the full transcript as memory;
- storing every transient turn detail as long-lived memory.

Memory and extension sources should also preserve the stable top-level context
prefixes. New repository context, skill, hook, agent, or memory inputs should
attach labels and provenance to structured source objects, not invent ad hoc
model-facing prefixes. In particular, transient runtime artifacts such as
orphan tool results should not be serialized back into ordinary user text with
synthetic labels.

## Hook-Driven Context Sources

Hooks can help context management, but they must not become hidden prompt
append points.

### Core Principle

Hook output is a context candidate, not final prompt text.

Every prompt-affecting hook result should enter the normal context pipeline as
a structured source object with:

- source id;
- lifecycle point;
- provenance;
- trust and authorship;
- time-to-live;
- budget hint;
- cache or invalidation status when applicable;
- human-readable inclusion or drop reason for `/context`.

The context assembler and selection layers remain the only owners of final
model-request injection. This keeps hook output budgeted, inspectable, and
stable across TUI, ACP, Wire, and future headless adapters.

### Injection Timing

Prompt injection must happen before model request assembly completes. Later
lifecycle events may produce candidates for future turns, but they must not
mutate the already assembled request.

The stable turn pipeline is:

1. collect raw inputs and runtime state;
2. run eligible pre-assembly hook discovery;
3. convert hook, memory, skill, prompt, and protocol inputs into context
   candidates;
4. run context and memory selection with ordering, deduplication, budget, and
   drop reasons;
5. assemble the final model request;
6. record tool and runtime events;
7. feed resulting context candidates into the next turn or compaction cycle.

This ordering avoids mid-stream prompt mutation, unstable source ordering, and
context that cannot be explained by `/context`.

### Lifecycle Responsibilities

`SessionStart` may discover session-level context such as environment facts,
workspace roots, adapter capabilities, and static extension declarations. If
the first model request must see that output, RARA should finish the relevant
session-start context work before assembling the first request. Late
session-start output must be marked as next-turn context instead of silently
patching an in-flight prompt.

`UserPromptSubmit` may attach structured metadata related to the submitted
input. It must not rewrite the user's original text. Any added context enters
selection as a candidate source.

Workspace-change hooks, when added, should invalidate prompt-source, memory,
skill, or environment caches. They should prefer dirty markers over direct
prompt injection.

`PreToolUse` is primarily for policy, permission, and environment checks. If it
discovers missing context, it should emit a warning, request, or candidate for a
future assembly step rather than changing the semantics of an already generated
tool call.

`PostToolUse` may emit context deltas such as touched files, generated
artifacts, test summaries, or important stderr summaries. These deltas belong
in thread/runtime state first. Whether they reach the model is decided by the
next assembly pass.

`PreCompact` may provide retain hints, such as files, decisions, constraints,
or failures that should survive compaction. Compaction remains owned by RARA;
hook output is advisory and must be visible in compaction/context
observability.

`Stop` may perform validation or final context reconciliation, but it must be
bounded and loop-safe. RARA should not run stop hooks on prompt-too-long or API
error paths that could create an error, hook block, retry, compact, error
cycle.

### Observability

`/context` should show hook-created context separately from ordinary prompt
sources and memory candidates, including:

- declared hook source and lifecycle;
- whether the hook is supported, ignored, executed, or deferred;
- selected hook context items;
- available but not injected hook items;
- dropped hook items and drop reasons;
- cache invalidations or retain hints emitted by hooks.

`/status` should summarize active hook support and last hook failures without
requiring users to inspect raw logs.

### Memory Scopes

RARA should distinguish at least two memory scopes.

#### Thread Memory

Thread memory is durable knowledge tied to a single thread/session lineage.

Examples:

- important earlier findings in the same debugging session;
- accepted local decisions for the current thread;
- compacted summaries of older thread history;
- temporary constraints that still matter for this thread.

Properties:

- local to the thread;
- may be dropped or archived with the thread;
- should be preferred over workspace memory when the current task is clearly
  continuing the same thread.

#### Workspace Memory

Workspace memory is durable knowledge that outlives one thread and can be
  reused across sessions.

Examples:

- repository-specific engineering conventions;
- long-lived user preferences relevant to this repo;
- stable project facts;
- recurring warnings about non-obvious architecture constraints.

Properties:

- shared across threads in the same workspace;
- more conservative write policy;
- should only store knowledge with clear reuse value.

### Memory Record Model

Memory should be stored as structured records rather than only as opaque text.

Suggested fields:

- `id`
- `scope`
  - `thread`
  - `workspace`
- `kind`
  - `decision`
  - `preference`
  - `repo_fact`
  - `bug_finding`
  - `warning`
  - `todo`
- `summary`
- `details`
- `tags`
- `source_thread_id`
- `confidence`
- `updated_at`

This metadata is important for:

- retrieval;
- ranking;
- promotion from thread memory to workspace memory;
- later cleanup or invalidation.

### Retrieval Flow

Memory should reach the model through a selection pipeline instead of direct
prompt dumping.

Recommended flow:

1. build the current `TurnContext`;
2. retrieve candidate records from `ThreadMemory`;
3. retrieve candidate records from `WorkspaceMemory`;
4. rank and filter the combined candidates;
5. shape the selected memory into compact context items;
6. inject only those shaped items into the assembled context.

### Selection Rules

Selection should consider:

- relevance to the current turn;
- scope preference:
  - thread memory before workspace memory when both are relevant;
- recency;
- confidence;
- token budget cost;
- duplication against active turn context and stable instructions.

### Shaping Rules

Memory records should usually be transformed before injection.

Examples:

- multiple related records may become one short memory summary;
- verbose historical notes may be reduced to a compact bullet list;
- low-confidence or stale records may be omitted entirely.

The final context should include memory as clearly labeled sections such as:

- relevant thread memory;
- relevant workspace memory.

### Write Policy

RARA should not write every turn into memory.

Memory writes should be limited to information with clear future value.

Good candidates:

- accepted architectural decisions;
- durable user preferences;
- important debugging findings;
- reusable repo facts;
- compacted thread summaries worth retaining.

Bad candidates:

- ordinary short-lived chat;
- routine command output;
- speculative notes that were never confirmed;
- ephemeral state that only mattered inside one immediate tool call.

### Promotion Policy

Not all thread memory should become workspace memory.

Suggested rule:

- write important short-to-medium-lived findings into thread memory first;
- promote only clearly reusable knowledge into workspace memory.

This reduces contamination of the workspace-level memory store.

### Relation to Compaction

Compaction and memory solve different problems.

- compaction
  - keeps a long thread within token budget;
- memory
  - retains reusable knowledge across turns or sessions.

Therefore:

- compaction summaries may be stored as thread memory;
- only selected durable facts from those summaries should be promoted into
  workspace memory.

Compaction should not automatically imply workspace-memory writes.

### Relation to Context Assembly

The final context for a turn should be assembled from:

1. stable instructions;
2. workspace context;
3. active turn context;
4. selected thread memory;
5. selected workspace memory;
6. compacted history;
7. backend/model-specific framing.

This keeps memory integrated with context while preserving a clean conceptual
boundary between the source of knowledge and the working set for a turn.

## Retrieval Backend Strategy

RARA should support multiple retrieval backends over time, but they should sit
behind the memory/retrieval boundary rather than leaking directly into prompt
assembly.

### Core Principle

Retrieval backends are implementation strategies for selecting memory and
context candidates. They are not the same thing as context itself.

RARA should therefore model:

- `ContextAssembler`
  - consumes already selected context items;
- `MemoryStore`
  - owns durable memory records;
- `Retriever`
  - produces candidate memory/context items from one or more retrieval
    backends.
- `RetrievalOrchestrator`
  - normalizes candidates from multiple source providers, applies cross-source
    ranking/dedupe inputs, and emits the structured provider/candidate view
    consumed by `/context`, ACP/Wire, and `MemorySelection`.

### Required Backend Classes

RARA should plan for at least three retrieval styles.

#### 1. Vector Retrieval

Vector retrieval is the default semantic-memory path.

Use cases:

- semantic recall of prior debugging findings;
- similar code/task history;
- project knowledge snippets;
- durable workspace memory lookup.

Requirements:

- embedding-backed nearest-neighbor search;
- metadata filters by scope, kind, recency, and thread/workspace identity;
- ranking and deduplication before prompt injection.

#### 2. Lance-Backed Storage

Lance/LanceDB should be the concrete local-first storage engine for vector
memory and related retrieval indexes.

Rationale:

- local-first fit;
- good structured metadata support;
- append/search semantics appropriate for thread/workspace memory records;
- room for future hybrid retrieval and analytics.

Expected role:

- persistent storage for `MemoryRecord`;
- vector index storage;
- metadata filtering;
- future hybrid retrieval experiments.

Lance is an implementation backend, not the public memory contract.

#### 3. Graph RAG

Graph RAG should be treated as a higher-level retrieval strategy layered on top
of durable memory and extracted relationships.

Use cases:

- architecture questions spanning many modules;
- dependency and ownership reasoning;
- connecting symbols, files, services, and decisions;
- traversing relationships rather than only retrieving semantically similar
  text.

Requirements:

- graph nodes for entities such as:
  - files
  - modules
  - symbols
  - threads
  - decisions
  - owners
- graph edges such as:
  - imports/depends on
  - implements
  - modified by
  - decided in
  - related to
- traversal/ranking layer that can produce compact, explainable context items.

Graph RAG should complement vector retrieval, not replace it.

### Retrieval Composition

RARA should eventually support a composed retrieval pipeline:

1. thread-scoped recall;
2. workspace vector recall;
3. graph traversal for relationship-heavy questions;
4. merge, rerank, dedupe, and trim;
5. inject only the shaped results into context.

### Suggested Abstractions

The runtime should converge on interfaces similar to:

- `MemoryStore`
  - read/write/update/delete memory records;
- `VectorIndex`
  - semantic candidate retrieval;
- `GraphIndex`
  - relationship traversal and neighborhood expansion;
- `Retriever`
  - orchestrates vector + graph + metadata ranking;
- `RetrievalSourceProvider`
  - produces typed candidates from one source family without deciding prompt
    injection;
- `RetrievalOrchestrator`
  - merges providers, preserves provenance, attaches drop reasons, and forwards
    normalized candidates to `MemorySelection`;
- `MemorySelection`
  - final selected records for the current turn.

This keeps prompt assembly independent from the specific storage/index choice.

### Routing Rules

RARA should not always run every retrieval backend on every turn.

Suggested routing:

- thread continuation / recent debugging:
  - thread memory first;
- semantic recall across prior work:
  - vector retrieval;
- repo architecture / dependency / ownership questions:
  - graph retrieval, optionally combined with vector recall;
- simple short turns:
  - skip expensive retrieval entirely.

### Data Flow

Recommended data flow:

1. raw thread/tool/session events are persisted;
2. selected durable facts are written as `MemoryRecord`;
3. `MemoryRecord` content is embedded and indexed into Lance-backed vector
   storage;
4. selected entities/relations are also written into a graph index when useful;
5. retrieval produces candidates;
6. candidates are shaped into compact context items;
7. `ContextAssembler` injects them into the final turn context.

### Phased Adoption

RARA should adopt these backends in phases.

#### Phase A

- real vector retrieval over Lance-backed storage;
- metadata-aware workspace/thread memory lookup.

#### Phase B

- hybrid retrieval:
  - vector + metadata + recency ranking;
- clearer promotion from thread memory to workspace memory.

#### Phase C

- graph extraction and graph-backed retrieval for architecture-heavy tasks;
- graph/vector composed retrieval.

### Constraints

- retrieval output must stay bounded by context budget;
- retrieval must remain explainable enough to debug;
- graph extraction should not block normal turns when graph data is missing;
- vector and graph stores should remain replaceable behind stable traits.

## Object Model

RARA should converge on explicit context-domain objects instead of spreading the
same data across prompt builders, TUI state, and agent history.

### StableContext

Represents always-on instruction state.

Suggested fields:

- base instructions;
- resolved instruction sources;
- runtime policy flags;
- provider/model runtime metadata needed every turn.

### WorkspaceContext

Represents durable workspace-scoped memory and repo facts.

Suggested fields:

- workspace root;
- durable workspace notes;
- retrieved memory snippets;
- optional structured facts derived from prior sessions.

### TurnContext

Represents the currently active turn.

Suggested fields:

- current user goal;
- active plan;
- pending approvals/questions;
- recent tool requests/results;
- current turn transcript items.

### ThreadContext

Represents persisted thread state.

Suggested fields:

- thread id and metadata;
- live rollout items;
- compacted summaries;
- session lineage/fork metadata;
- archive/resume state.

### ContextBudget

Represents the token policy for the selected model/backend.

Suggested fields:

- `context_window_tokens`;
- `reserved_output_tokens`;
- `compact_threshold_tokens`;
- `stable_instructions_budget`;
- `workspace_prompt_budget`;
- `active_turn_budget`;
- `compacted_history_budget`;
- `retrieved_memory_budget`;
- `remaining_input_budget`;
- optional backend-specific margins.

## Persistence Boundary

RARA should add a thread/session persistence boundary similar in spirit to
Codex's `thread-store`, but adapted to RARA's current local-first architecture.

### Required Trait Boundary

RARA should eventually expose two main persistence abstractions:

- `ThreadStore`
  - create/read/list/archive/resume/update metadata
- `ThreadRecorder`
  - append rollout items
  - flush
  - shutdown

### Why This Matters

This separates:

- thread lifecycle;
- live append behavior;
- prompt assembly inputs;
- compaction outputs.

Without this boundary, compaction and resume continue to operate on partial or
flattened state.

### First-Pass Scope

The first pass only needs a local implementation backed by RARA's existing
local storage paths.

Remote-backed thread storage is explicitly follow-up work.

## Rollout Item Model

Thread history should be persisted as structured rollout items rather than raw
chat strings.

At minimum, the model should represent:

- user input;
- assistant text;
- tool use;
- tool result;
- plan updates;
- pending interactions;
- compaction events;
- warnings/errors that affect later turns.

This keeps resume, TUI rendering, compaction, and analytics aligned to the same
underlying source of truth.

## Context Assembly Contract

Prompt assembly should become a dedicated step that consumes structured context
objects and produces backend-specific input.

### Assembly Order

Recommended order:

1. stable instructions
2. workspace context
3. active turn context
4. compacted history
5. backend/model-specific framing

### Assembly Invariants

- active turn items must not be silently compacted;
- stable instructions must not be derived from chat history;
- compacted history must remain explicitly marked as compacted;
- workspace memory must not be confused with thread history;
- backend-specific rendering should happen after context selection, not during
  context discovery.
- the assembly pass must emit an ordered, explainable source list that can be
  consumed unchanged by agent debugging surfaces such as `/status` and
  `/context`;
- dropped or retrieval-ready items must remain attributable with an explicit
  non-injection reason instead of disappearing silently.

### Stage 1 Checkpoint

The first Stage 1 landing does not replace the entire prompt-send path with a
new message renderer. Instead, it centralizes ownership of the effective
context explanation and budget accounting around one assembly boundary.

Stage 1 now requires:

- one `ContextAssembler` entrypoint that produces:
  - the effective prompt/runtime view;
  - a `ContextBudget`-shaped breakdown;
  - ordered assembly entries with inclusion reasons and dropped-item reasons;
- one assembler-owned turn result object so agent/runtime callers can read the
  prompt view and the runtime/debug view from the same turn contract instead of
  rebuilding them through separate helper paths;
- `SharedRuntimeContext` and TUI runtime snapshots to carry that structured
  assembled view directly;
- `/status` and `/context` to consume the same assembly result instead of
  rebuilding their own prompt/context explanation in parallel;
- session restore to rebuild the same assembled-context view from persisted
  thread/runtime state, including:
  - plan state;
  - pending interactions;
  - compacted history inputs.

This keeps retrieval behavior and the current model-send path intact while
moving final context ownership behind an explicit assembly object.

## Compaction Model

Compaction should be a first-class lifecycle event.

### Trigger Conditions

Compaction may be triggered by:

- token budget exceeded;
- pre-turn budget check;
- manual user request;
- background maintenance for very long threads.

### Inputs

Compaction should operate on:

- persisted structured thread items;
- current context budget;
- the subset of history eligible for replacement.

### Outputs

Compaction should produce:

- a compacted summary item;
- metadata describing what history range was replaced;
- before/after token counts;
- a persisted compaction event.

### Invariants

- recent active items remain uncompressed;
- compacted summaries stay attributable to their original history;
- compaction never mutates stable instructions;
- compaction must be visible to resume, analytics, and TUI status surfaces.

## Model-Aware Budgeting

RARA should stop using one generic context heuristic across all backends.

Each backend/model should be able to provide a `ContextBudget`.

### Required Inputs

- model context window;
- reserved response/output budget;
- optional auto-compact threshold;
- optional model-specific overhead assumptions.

### Expected Sources

- Codex/provider model metadata where available;
- local model spec metadata for Candle backends;
- explicit RARA defaults only as fallback when the provider cannot supply
  better data.

## TUI / Runtime Implications

The TUI should present context state as runtime objects, not as ad hoc
transcript side effects.

Examples:

- active plan from `TurnContext`;
- pending approval from structured pending interactions;
- compaction notices from persisted compaction events;
- status/footer token usage from `ContextBudget` and recorded token accounting.

This reduces duplication between:

- transcript rendering;
- footer/runtime status;
- session resume;
- compaction reporting.

## Plan Mode Architecture

Plan mode should be modeled as structured runtime state, not as a loose
assistant prose convention.

### Core Principle

Planning is not just a different prompt. It is a different turn state with its
own lifecycle, render objects, persistence requirements, and transition rules.

### Required Plan Objects

RARA should represent planning state explicitly through objects such as:

- `PlanningState`
  - whether the current turn is in planning mode;
  - plan explanation/summary;
  - current structured steps;
  - approval requirement;
  - plan origin/source metadata.
- `PlanStep`
  - `pending`
  - `in_progress`
  - `completed`
  - optional `failed` or `needs_retry`
- `PendingInteraction`
  - plan approval
  - planning question
  - exploration question

### Persistence Contract

Plan updates must be persisted as structured rollout items, not reconstructed
from the assistant's natural-language transcript afterward.

At minimum, persisted plan events should include:

- plan creation/update;
- step status transitions;
- plan approval requested;
- plan approval accepted/rejected;
- planning failure/interruption.

### Rendering Contract

The TUI should render planning from structured state:

- `Planning`
  - short sidecar summary only;
- `Updated Plan`
  - structured checklist from `PlanStep`;
- `Awaiting Approval`
  - explicit interaction card;
- runtime heartbeat
  - status/footer only, not mixed into plan transcript.

This avoids:

- duplicated `Plan Decision` cards;
- generic `Working` fallback during planning;
- accidental completion of plan steps after partial failures;
- raw planning prose being shown as if it were stable state.

### Plan Transitions

The runtime should enforce explicit transitions:

1. `execute -> planning`
2. `planning -> awaiting approval`
3. `awaiting approval -> execute`
4. `awaiting approval -> continue planning`
5. `planning/execute -> interrupted`
6. `planning/execute -> compacted and resumed`

### Context Rules for Plan Mode

When building context for a planning turn:

- include stable instructions and workspace context as usual;
- include the active planning state as structured context;
- include recent exploration/tool results needed for the plan;
- avoid replaying raw planning prose once it has been captured into structured
  plan objects.

### Follow-Up Requirement

Plan-mode entry heuristics should eventually be moved away from keyword-style
rules and toward:

- runtime state;
- new-task vs follow-up classification;
- explicit pending-interaction context.

## Sub-Agent Architecture

Sub-agents should be modeled as child threads with explicit parent/child
relationship, not as anonymous assistant tool chatter.

### Core Principle

A sub-agent invocation is a structured delegation event:

- it starts a child thread;
- it has an explicit task contract;
- it produces structured outputs back to the parent;
- it may contribute summaries back into the parent thread context.

### Current Runtime Checkpoint

The current runtime has three direct sub-agent tools:

- `spawn_agent`: a no-tool reasoning worker;
- `explore_agent`: a read-only repository inspection worker;
- `plan_agent`: a read-only planning worker.

`team_create` is a synchronous aggregation tool over the same sub-agent runtime.
It accepts multiple task contracts and runs them concurrently inside one tool
call, then returns one ordered `team_results` array to the parent. The current
runtime accepts at most 8 tasks and runs at most 4 sub-agents at once. This
matches the foreground delegation boundary.

Foreground sub-agent invocations now generate an `agent_id`, persist completed
child history as a parent-scoped sidechain transcript, append a `SpawnAgent`
rollout event with parent/child edge metadata, and return only a structured
delegation result to the parent context. Background sub-agent invocations use
the same contract but return immediately with `status = running`; the live
runtime keeps an in-process task registry for `subagent_list`,
`subagent_resume`, and `subagent_stop`. These tools expose only edge/status
metadata and final summaries to the parent, not the child sidechain transcript.
Cross-process restart/reattach remains a separate durable task-registry layer.

### Required Objects

RARA should represent delegation with objects such as:

- `SubAgentTask`
  - task id;
  - parent thread id;
  - child thread id;
  - instruction/task payload;
  - ownership/write-scope metadata;
  - current status.
- `SubAgentResult`
  - summary;
  - changed files or produced artifacts;
  - validation status;
  - failure reason if any.
- `SubAgentContextPolicy`
  - what parent context is inherited;
  - what child outputs are folded back into parent context;
  - what is persisted only in the child thread.

### Parent/Child Thread Model

Sub-agents should be backed by the same `ThreadStore` boundary as main threads.

This means:

- each sub-agent gets its own thread id;
- child rollout items are persisted independently;
- parent thread stores delegation events and returned summaries;
- resume/fork/archive can preserve lineage.

### Context Inheritance Rules

RARA should not blindly copy the full parent history into every child.

Instead, a sub-agent should receive:

- stable instructions;
- the minimal workspace context required for the task;
- the delegation instruction;
- optionally the relevant compacted thread summary;
- only the recent active-turn items needed for the delegated work.

This keeps child context small and avoids runaway token usage.

### Return Path

When a sub-agent completes, the parent should not ingest the entire child
transcript.

Instead, the parent should receive a structured return item containing:

- short summary;
- relevant outputs or changed files;
- validation result;
- optional follow-up suggestions.

If the child generated important durable knowledge, that can be stored in:

- thread history as a delegation result;
- workspace context as durable memory;
- both, if needed.

### TUI Implications

The TUI should render sub-agents as explicit delegation objects, for example:

- `Delegated`
  - child task summary;
- `Sub-agent update`
  - progress summary;
- `Sub-agent completed`
  - result summary.

This should be rendered from structured rollout items rather than from raw
assistant prose like "I will ask another agent...".

### Compaction Rules

Sub-agent history should be compacted per child thread, not folded into the
parent as raw transcript.

The parent thread should only retain:

- the delegation request;
- the child result summary;
- any explicitly promoted durable facts.

### Staged Rollout

Sub-agent support should likely be introduced after Stage 2 of this document,
because it depends on:

- a real thread store;
- structured rollout items;
- explicit parent/child lineage.

Before that, RARA can still support local delegation, but it should be treated
as transitional runtime behavior rather than the final context architecture.

## Staged Rollout Plan

### Stage 1: Context Budget and Assembly Boundary

Introduce:

- `ContextBudget`
- `ContextAssembler`
- explicit separation of stable/workspace/active/compacted inputs

Goal:

- prompt assembly becomes deterministic without immediately replacing all
  persistence.

### Stage 2: Local Thread Store

Introduce:

- local `ThreadStore`
- local `ThreadRecorder`
- structured rollout item persistence

Goal:

- resume and history reconstruction stop depending on flattened transcript
  recovery.

### Stage 3: First-Class Compaction

Introduce:

- structured compaction events;
- compacted history replacement;
- before/after token accounting tied to thread persistence.

Goal:

- long sessions remain stable without replaying unbounded raw history.

### Stage 4: Full Runtime Integration

Integrate thread/context objects with:

- TUI status and transcript rendering;
- session resume/fork/archive;
- memory/retrieval selection;
- provider-specific budgeting and model metadata.

Goal:

- one shared context model across runtime, persistence, and UI.

### Stage 5: Plan and Delegation Unification

Integrate:

- structured plan state;
- pending interactions;
- sub-agent child-thread support;
- parent/child context inheritance and result folding.

Goal:

- planning, execution, delegation, compaction, and resume all operate on the
  same thread/context model instead of parallel ad hoc state.

## Validation

Non-trivial rollout stages should add focused tests for:

- instruction precedence and assembly order;
- budget calculation for multiple backends/models;
- compaction trigger and replacement behavior;
- thread resume after compaction;
- TUI rendering from structured pending/plan/compaction state;
- plan step persistence and approval transitions;
- parent/child sub-agent thread lineage and result folding;
- provider/model-specific budget changes.

## Follow-Up Work

- Define the concrete `ThreadStore` local schema and rollout-item format.
- Decide how much of existing session/state DB can be reused versus replaced.
- Integrate retrieval/memory selection cleanly into `WorkspaceContext`.
- Add model/provider metadata plumbing so all backends can emit real
  `ContextBudget` values.
- Define the concrete parent/child thread contract for sub-agent delegation.
- Replace transitional plan/transcript heuristics with structured plan-state
  persistence end to end.
