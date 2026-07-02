//! Staged context architecture module.
//!
//! Stage 1 introduces the shared assembly boundary while keeping the module
//! lightweight enough for branches that only need `mod context;` to compile.

mod assembler;
mod assembly_view;
mod compaction_view;
mod file_search_provider;
mod memory_selection;
mod retrieval_provider;
mod retrieval_view;
mod retrieved_memory_render;
mod retriever;
mod runtime;

pub use self::assembler::{
    AssembledContext, AssembledTurnContext, ContextAssembler, RuntimeContextInputs,
    RuntimeInteractionInput,
};
pub(crate) use self::file_search_provider::FileSearchCandidateProvider;
pub(crate) use self::retrieval_provider::{
    McpResourceReference, RetrievalRequest, mcp_resource_candidate, retrieval_candidates,
};
pub(crate) use self::retrieved_memory_render::{
    RetrievedMemoryRenderItem, render_retrieved_memory_context,
    render_retrieved_memory_context_item,
};
pub(crate) use self::retriever::MemoryRetrievalOrchestrator;
pub use self::runtime::{
    AgentTurnTraceView, CacheStatus, CompactionContextView, CompactionSourceContextEntry,
    ContextAssemblyEntry, ContextAssemblyView, ContextBudgetView, ContextCacheObservationView,
    ContextCompactionObservationView, ContextObservabilityView, DropReason,
    MemorySelectionContextView, MemorySelectionItemContextEntry, MicrocompactProjectionContextView,
    PlanContextView, PromptContextView, PromptSourceContextEntry, RETRIEVED_THREAD_CONTEXT_KIND,
    RETRIEVED_WORKSPACE_MEMORY_KIND, RetrievalBudgetContextView, RetrievalCandidate,
    RetrievalCandidateContextEntry, RetrievalContextView, RetrievalObservationView,
    RetrievalOrchestrationView, RetrievalProviderStatus, RetrievalSourceContextEntry,
    RetrievalSourceRef, RetrievedMemoryCandidate, SharedRuntimeContext, SharedTaskContextItem,
    SharedTaskContextView, TodoContextView, is_retrieved_memory_kind,
};
