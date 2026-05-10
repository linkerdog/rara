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

use crate::agent::Agent;
use crate::hook_registry::HookRegistry;
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
/// - Session/Input requests → `Agent` (when provided)
pub async fn dispatch<F>(
    envelope: RuntimeControlEnvelope,
    mcp_manager: &McpConnectionManager,
    prompt_registry: &PromptSourceRegistry,
    skill_registry: &SkillSourceRegistry,
    memory_handler: &MemoryControlHandler,
    hook_registry: &HookRegistry,
    agent: Option<&mut Agent>,
    mut on_event: F,
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
            prompt_registry
                .handle_control_with_provenance(prompt_request, envelope.provenance.clone())
                .await;
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
        RuntimeControlRequest::Hook(hook_request) => {
            hook_registry.handle_control(hook_request).await;
            // Callback wiring for in-process hooks is handled by the hook
            // loader (hooks.rs) — not by the control-plane dispatcher.
            Ok(())
        }
        RuntimeControlRequest::Session(session_request) => {
            if let Some(agent) = agent {
                agent
                    .handle_session_control(session_request)
                    .await
                    .map_err(|err| err.to_string())
            } else {
                Err("no active session available for session control".to_string())
            }
        }
        RuntimeControlRequest::Input(input_request) => {
            if let Some(agent) = agent {
                // Bridge AgentEvent back to the control-plane callback via on_event.
                // We need to wrap AgentEvent into RuntimeControlEvent.
                let mut sequence = 0u64; // We might need the bus sequence here
                let provenance = envelope.provenance.clone();

                let mut report = |event| {
                    sequence += 1;
                    let event_id = format!("evt-dispatch-{}", sequence);
                    let control_event = crate::runtime_control::wrap_agent_event(
                        event_id,
                        sequence,
                        provenance.clone(),
                        event,
                    );
                    on_event(control_event);
                };

                agent
                    .handle_input_control(input_request, &mut report)
                    .await
                    .map_err(|err| err.to_string())
            } else {
                Err("no active session available for input control".to_string())
            }
        }
        _ => Err("control-plane dispatch not yet implemented for this request variant".into()),
    }
}
