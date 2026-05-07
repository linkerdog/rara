# Retrieval Orchestration Spec

Added a dedicated retrieval orchestration spec to separate three concepts that
were previously easy to blur:

- source providers that produce raw recall candidates;
- orchestration that normalizes, ranks, dedupes, and explains candidates;
- `MemorySelection` that performs final selected/available/dropped decisions
  against the current turn budget.

The spec records the current implementation checkpoint:

- `MemoryRetrievalOrchestrator` already retrieves LanceDB-backed workspace
  memory and session shard candidates.
- `memory_selection()` already reports selected, available, dropped, and budget.
- `SharedRuntimeContext` already carries a retrieval view for `/context`.

Implemented the first slice after the spec:

- added `RetrievalSourceRef` and `RetrievalCandidate` as the typed boundary;
- adapted current direct memory/session retrieval inputs through that boundary
  before `MemorySelectionCandidate`;
- kept prompt injection and selected/available/dropped behavior unchanged;
- added focused tests for candidate metadata and deterministic selection order.

The next implementation slice should add an orchestration view with provider
status and candidate-level explainability for `/context` and future ACP/Wire
consumers.

Implemented the second thin slice:

- added `RetrievalOrchestrationView` with provider status, selected, available,
  dropped, combined candidates, and budget rollups;
- derived the view from existing retrieval source status plus
  `MemorySelectionContextView`;
- carried the view through `SharedRuntimeContext` and the TUI runtime snapshot;
- switched `/context` to render provider and candidate groups from the
  orchestration view;
- added deterministic candidate ordering and dedupe by retrieval dedupe key;
- added an ACP/Wire-ready `ContextEvent::RetrievalOrchestrationUpdated`
  contract carrying the same orchestration view;
- kept prompt assembly unchanged.

Auxiliary/lite-model compaction is explicitly deferred until context
observability is complete. The orchestration view only leaves enough structure
for a future compression capability to preserve original candidate detail and
report summary provenance if that work is picked up later.
