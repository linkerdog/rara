# Retrieval Orchestration

Retrieval orchestration is the runtime layer that turns many possible context
sources into a small, ranked, explainable candidate set for `MemorySelection`.

It is deliberately separate from storage backends, prompt assembly, and final
selection. Storage systems find raw material. Orchestration normalizes and
ranks candidates. `MemorySelection` decides what fits the current turn budget.

## Problem

RARA already has several recall-like sources:

- workspace memory prompt sources;
- LanceDB-backed `MemoryRecord` search;
- per-session context shard search;
- compacted history source descriptors;
- active turn state such as plans, pending interactions, latest request, and
  recent tool results;
- future file-search candidates from `crates/file-search`;
- future MCP resources, hook output, and protocol-registered sources.

Without a retrieval orchestration boundary, each source can leak directly into
prompt assembly with its own ranking, dedupe, budget, and debug story. That
makes `/context` less useful and makes ACP/Wire integration hard because there
is no single structured candidate view to expose.

## Scope

This spec defines the target boundary and rollout path for a unified retrieval
orchestration layer.

In scope:

- normalize memory-like and context-like source hits into typed candidates;
- preserve provenance, scope, source id, ranking inputs, and budget estimates;
- merge candidates from thread, workspace, session shard, file search, MCP,
  hook, and future graph/vector sources;
- provide deterministic ordering and explicit reasons for selection,
  availability, and drops;
- keep `/context`, `/status`, ACP, and Wire reading the same structured view.

Out of scope for this spec:

- implementing every backend source at once;
- replacing `MemoryStore`, `ThreadStore`, or LanceDB internals;
- changing the stable prompt prefix;
- persisting volatile retrieval candidates as ordinary chat history;
- auto-injecting file-search results before they pass through
  `MemorySelection`.

## Architecture

The target pipeline has five stages:

1. **Source discovery**
   - Collect source providers available for the turn.
   - Examples: active prompt sources, `MemoryStore`, session context shards,
     file-search provider, MCP resource provider, hook context provider.

2. **Candidate production**
   - Each provider returns `RetrievalCandidate` values.
   - Providers own source-specific search and metadata extraction.
   - Providers do not decide prompt injection.

3. **Orchestration**
   - Merge candidates.
   - Normalize scores and priorities.
   - Apply source ownership rules and dedupe keys.
   - Attach budget estimates and control-plane visibility fields.

4. **Selection**
   - Pass normalized candidates to `MemorySelection`.
   - `MemorySelection` decides selected, available, and dropped items based on
     fixed context, ranking, dedupe, and remaining budget.

5. **Assembly**
   - `ContextAssembler` injects selected volatile retrieval only in the volatile
     suffix near the latest user request.
   - Stable prompt prefixes remain byte-stable.

Current code already has part of this shape:

- `MemoryRetrievalOrchestrator` retrieves workspace memory and session context
  candidates;
- `RetrievalCandidate` and `RetrievalSourceRef` provide the first typed
  candidate boundary for direct memory/session retrieval inputs;
- `memory_selection()` ranks selected/available/dropped entries;
- `SharedRuntimeContext.retrieval.memory_selection` is consumed by `/context`.

The remaining missing layer is a source-provider contract and orchestration view
shared by current memory retrieval and future file/MCP/hook/graph sources.

## Candidate Contract

Target candidate shape:

```rust
pub struct RetrievalCandidate {
    pub id: String,
    pub source: RetrievalSourceRef,
    pub kind: RetrievalCandidateKind,
    pub scope: RetrievalScope,
    pub label: String,
    pub detail: String,
    pub summary: Option<String>,
    pub rank: usize,
    pub score: Option<f32>,
    pub priority: usize,
    pub dedupe_key: Option<String>,
    pub budget_impact_tokens: Option<usize>,
    pub selection_reason: String,
    pub availability_reason: String,
}
```

Required source fields:

- stable source type, such as `memory_record`, `session_context`, `file_search`,
  `mcp_resource`, `hook_output`, `graph_context`;
- source id, if durable;
- source path or URI, if available;
- session id / thread id / workspace id where applicable;
- freshness metadata such as created/updated time when available.

Candidates must be deterministic:

- same source inputs produce the same `id`, `dedupe_key`, and ordering;
- ties are broken by source priority, rank, then stable label/id;
- hash maps must not define externally visible ordering.

## Source Providers

Target trait shape:

```rust
pub trait RetrievalSourceProvider {
    fn source_kind(&self) -> &'static str;

    async fn candidates(
        &self,
        request: &RetrievalRequest,
    ) -> anyhow::Result<Vec<RetrievalCandidate>>;
}
```

`RetrievalRequest` should include:

- latest user request text;
- current session/thread/workspace ids;
- execution mode;
- budget hints;
- enabled source classes;
- optional query embedding;
- current stable prompt-source manifest;
- recent active-turn tool/result manifest.

Provider responsibilities:

- `MemoryStoreProvider`: search workspace/thread `MemoryRecord`s through
  LanceDB-backed APIs.
- `SessionShardProvider`: search per-session context shards.
- `FileSearchProvider`: produce file candidates from `crates/file-search`;
  it must not inject file contents directly.
- `McpResourceProvider`: surface referenced MCP resources as candidates.
- `HookContextProvider`: surface hook output as volatile candidates.
- `GraphProvider`: later graph/vector composed context.

## Ranking And Dedupe

Orchestration owns cross-source ordering inputs. `MemorySelection` owns final
budget decisions.

Initial priority order:

1. active fixed context already selected by owner layers;
2. focused session/thread context relevant to the current request;
3. workspace memory records;
4. file candidates;
5. MCP resource candidates;
6. hook output;
7. graph expansion candidates until graph confidence is proven.

Initial dedupe rules:

- compacted thread history beats raw thread history;
- focused session context beats raw thread history;
- selected workspace prompt memory beats duplicate retrieved workspace memory;
- a memory record beats a file candidate when both point to the same durable
  fact;
- exact same source id and content hash dedupe to the highest-priority
  candidate;
- file candidates dedupe by canonical path;
- MCP candidates dedupe by resource URI.

Dropped candidates must retain the winning reason:

- `deduped_by=<candidate id>`;
- `budget_exceeded`;
- `source_disabled`;
- `below_score_threshold`;
- `stale_or_superseded`;
- `already_in_stable_prompt`;
- `covered_by_compaction`.

## Budget And Cache Stability

Stable prefix order must remain:

1. system prompt;
2. tool schemas;
3. stable skills and project memory;
4. compacted history and carry-over;
5. retrieval and volatile recent context;
6. latest user input.

Retrieved candidates belong in the volatile suffix unless explicitly promoted to
workspace memory.

Budget rules:

- fixed selected items can report cost but do not consume retrieval budget;
- volatile retrieval candidates consume `retrieved_memory_budget`;
- file-search candidates should initially charge only manifest text, not file
  contents;
- selected file contents, when added later, must charge their rendered excerpt
  cost;
- per-source budgets should be explicit so one backend cannot starve all other
  sources silently.

## Future Compression Boundary

Retrieval orchestration may later ask a compression stage to produce compact
candidate summaries, but that is not part of the current rollout and must stay
outside the stable prompt prefix.

Do not start auxiliary/lite-model compaction until context observability is
complete enough to show provider status, candidate status, budget usage, drop
reasons, and injected summary provenance in `/context` and the future OTEL
event stream.

The first observability boundary is now shared with context compression:
`ContextObservabilityView` summarizes retrieval provider count, candidate
count, selected/available/dropped counts, and budget token totals. Detailed
candidate explanations remain in `RetrievalOrchestrationView`; the
observability view is the stable summary shape for `/context`, ACP/Wire, and
future OTEL export.

Rules:

- keep compression behind an explicit provider capability and feature gate;
- keep the original candidate detail available for `/context` and auditability;
- record whether a candidate summary was generated by the main model,
  auxiliary model, or a deterministic local compactor;
- never use compression output to overwrite durable memory records unless the
  user explicitly requests promotion or memory update.

If this becomes necessary later, reuse the existing auxiliary-model routing
instead of creating a separate `lite` model concept inside retrieval.

## Runtime And Control Plane Contract

The orchestration result should be a structured runtime artifact:

```rust
pub struct RetrievalOrchestrationView {
    pub request_id: String,
    pub query: String,
    pub providers: Vec<RetrievalProviderStatus>,
    pub candidates: Vec<RetrievalCandidateView>,
    pub selected: Vec<RetrievalCandidateView>,
    pub available: Vec<RetrievalCandidateView>,
    pub dropped: Vec<RetrievalCandidateView>,
    pub budget: RetrievalBudgetView,
}
```

This view must be readable by:

- `/context`;
- `/status` summary;
- ACP/Wire subscribers;
- future OTEL exporters.

Protocol adapters must not concatenate raw retrieved text into prompts. They may
register source providers or request retrieval, but final injection stays behind
`MemorySelection` and `ContextAssembler`.

## `/context` Display Contract

`/context` should answer:

- which providers ran;
- which providers were skipped and why;
- which candidates were produced;
- which candidates were selected;
- which candidates were available but omitted;
- which candidates were dropped by dedupe, ranking, or budget;
- how much budget selected candidates consumed;
- whether the injected material is stable or volatile.

The display should keep details compact and path/URI-like fields visible.

## Phased Rollout

### Phase 1: Shape The Candidate Boundary

- Status: implemented for current direct memory/session retrieval candidates
  and the first provider boundary.
- `RetrievedMemoryCandidate` now adapts through `RetrievalCandidate` before it
  becomes a `MemorySelectionCandidate`.
- `RetrievalRequest` and `RetrievalSourceProvider` are now the source-provider
  boundary for current in-process retrieval sources.
- Current direct memory retrieval, retrieval tool results, thread history,
  vector-store slot, and file-search candidates all normalize into
  `RetrievalCandidate` before discretionary `MemorySelection`.
- Existing memory/session retrieval behavior remains unchanged.
- Unit tests cover typed boundary fields and deterministic selection order.

### Phase 2: Orchestration View

- Status: implemented for structured runtime state and `/context` display.
- Add `RetrievalOrchestrationView` to `SharedRuntimeContext`.
- Make `/context` read richer provider/candidate details from that view.
- Keep `/status` as summary only.
- Emit ACP/Wire-ready structured events from the same view.

### Phase 3: File Candidate Provider

- Status: first candidate adapter implemented.
- Add `FileSearchProvider` using `crates/file-search`.
- Return file manifest candidates only.
- Do not inject file contents until a later excerpt-selection step exists.

### Phase 4: Source-Specific Budgets And Dedupe

- Status: initial deterministic dedupe is implemented for current retrieval
  candidates.
- Add per-source budget caps.
- Add durable source ids and dedupe keys for future providers.
- Add tests for duplicate memory/file/session candidates.

### Phase 5: Graph/MCP/Hook Sources

- Add MCP resource candidates.
- Add hook output candidates.
- Add graph candidates once graph index confidence is sufficient.
- Reuse the same `RetrievalSourceProvider` and `RetrievalCandidate` contract;
  do not add source-specific inputs to `MemorySelection`.

## Validation Matrix

- Provider failures are visible but do not fail the whole turn unless the
  provider is required.
- Candidate order is deterministic with identical inputs.
- Dedupe preserves the winning candidate and records the losing reason.
- Tight budget drops candidates with explicit `budget_exceeded` reason.
- `/context` shows provider status, selected, available, dropped, and budget.
- ACP/Wire view uses the same structured data as `/context`.
- Retrieved candidates are not persisted as ordinary chat history.
- Stable prompt-prefix bytes are unchanged when only volatile retrieval changes.

## Open Risks

- Score normalization across vector, keyword, graph, and file-search sources can
  become misleading. Prefer source priority plus local rank until enough data
  exists.
- File-search candidates may tempt agents to inject too many files. Keep file
  contents behind a separate excerpt-selection step.
- Provider failure handling needs care: missing optional retrieval should be
  visible, not noisy.
- Per-source budget caps can hide useful context if defaults are too strict.

## Source Journals

- `docs/journal/2026-05-07-retrieval-orchestration-spec.md`
