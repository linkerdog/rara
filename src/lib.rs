#![recursion_limit = "512"]
#![allow(unused_imports)]

mod acp;
mod agent;
mod agents_ext;
mod app_cli;
mod auto_memory;
mod classifier;
mod codex_model_catalog;
mod config;
mod context;
mod control_plane;
mod control_tokens;
pub mod deepseek_cache_probe;
mod exec_consumer;
mod google_oauth;
mod hook_registry;
mod hook_runtime;
mod hooks;
mod llm;
mod local_backend;
mod lsp_manager;
mod mcp_connection_manager;
mod mcp_status;
mod mcp_tool_cache;
mod memory_distiller;
mod memory_files;
mod memory_lifecycle;
mod memory_notice;
mod memory_store;
mod model_context;
mod model_observation;
mod oauth;
mod plugin_cli;
mod plugin_middleware;
mod print_consumer;
mod prompt;
mod protocol_sources;
mod runtime_client;
mod runtime_context;
mod runtime_control;
mod runtime_event_bus;
mod runtime_goal;
pub mod runtime_session;
mod sandbox;
mod session;
mod session_context;
mod session_promotion;
mod session_transcript;
mod shell_env;
mod skill;
mod tasklist;
mod thread_cli;
mod thread_store;
mod todo;
mod tool_result;
mod tools;
mod tui;
mod utils;
mod wire_consumer;
mod workspace;

pub mod embedded;

pub use agent::{AgentEvent, AgentOutputMode};
pub use config::{MultiAgentPolicy, RaraConfig};
pub use deepseek_cache_probe::{
    DEFAULT_DEEPSEEK_CACHE_PROBE_MODEL, DeepseekCacheProbeArm, DeepseekCacheProbeOptions,
    DeepseekCacheProbeReport, DeepseekCacheProbeSample, DeepseekCacheProbeSummary,
    run_deepseek_cache_probe,
};
pub use embedded::{EmbeddedRuntime, EmbeddedRuntimeOptions};
pub use llm::{
    ContentBlock, ContextBudget, LlmBackend, LlmExecutionMode, LlmResponse, LlmStreamEvent,
    LlmTurnMetadata, Message, ProviderCacheProfile, TokenUsage,
};
pub use model_observation::{
    ModelCacheUsage, ModelRequestFingerprint, ModelTokenUsage, ModelTurnReport, QueryReport,
};
pub use rara_tools::tool::{
    Tool, ToolCallContext, ToolError, ToolManager, ToolOutputStream, ToolProgressEvent,
};
pub use runtime_control::{
    AssistantEvent, ErrorEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent,
    ToolEvent, ToolStream, WarningEvent,
};
pub use runtime_session::{
    RuntimeEventStream, RuntimeHost, RuntimeSession, RuntimeSessionBuilder, RuntimeSessionError,
    RuntimeSessionId, RuntimeSessionPhase, RuntimeSessionProfile, RuntimeSessionSnapshot,
    RuntimeSessionSubscription, RuntimeTurn, RuntimeTurnId, RuntimeTurnOutcome,
};
pub use tools::agent::{
    AgentMailboxMessage, AgentSnapshot, AgentTreeConfig, AgentTreeControl, AgentWaitResult,
};

/// Run the standard RARA CLI using the same runtime assembly exposed to
/// embedding applications.
pub async fn run_cli() -> anyhow::Result<()> {
    app_cli::run_cli().await
}
