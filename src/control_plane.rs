//! Control-plane dispatch that routes structured `RuntimeControlEnvelope`
//! messages to the appropriate domain handlers (agent, MCP, memory, etc.).
//!
//! This module is the single entry point for all external protocol adapters
//! (ACP, Wire, app-server) to interact with the RARA runtime.  It replaces
//! the older pattern of directly calling agent / tool / MCP APIs from
//! transport-specific code.
//!
//! Domain routing is implemented incrementally as individual control-plane
//! families are wired.

use crate::mcp_connection_manager::McpConnectionManager;
use crate::protocol_sources::{MemoryControlHandler, PromptSourceRegistry, SkillSourceRegistry};
use crate::runtime_control::{RuntimeControlEnvelope, RuntimeControlEvent, RuntimeControlRequest};

/// Dispatch a structured control-plane request and stream resulting events
/// to the provided callback.
///
/// Routes to the appropriate domain handler:
/// - MCP requests → `McpConnectionManager`
/// - Prompt/Skill source requests → respective registries
/// - Memory requests → `MemoryControlHandler`
pub async fn dispatch<F>(
    envelope: RuntimeControlEnvelope,
    mcp_manager: &McpConnectionManager,
    prompt_registry: &PromptSourceRegistry,
    skill_registry: &SkillSourceRegistry,
    memory_handler: &MemoryControlHandler,
    _on_event: F,
) -> Result<(), String>
where
    F: FnMut(RuntimeControlEvent) + Send,
{
    match &envelope.request {
        RuntimeControlRequest::Mcp(mcp_request) => {
            mcp_manager.handle_control(mcp_request).await;
            Ok(())
        }
        RuntimeControlRequest::PromptSource(prompt_request) => {
            prompt_registry.handle_control(prompt_request).await;
            Ok(())
        }
        RuntimeControlRequest::SkillSource(skill_request) => {
            skill_registry.handle_control(skill_request).await;
            Ok(())
        }
        RuntimeControlRequest::Memory(memory_request) => memory_handler
            .handle_control(memory_request)
            .await
            .map_err(|err| err.to_string()),
        _ => Err("control-plane dispatch not yet implemented for this request variant".to_string()),
    }
}
