use std::sync::{Arc, atomic::AtomicBool};

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::Agent;
use crate::hook_runtime::HookRuntime;
use crate::runtime_event_bus::RuntimeEventBus;

#[test]
fn new_thread_discards_private_turn_state_and_old_hook_output() {
    let (_dir, sessions, workspace, root) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(MemoryHandle::new(
            &root.join("memory").display().to_string(),
        )),
        sessions,
        workspace,
    );
    agent.stable_tool_schemas = Some(vec![json!({"name": "old_tool"})]);
    agent.recent_tool_calls = vec![("old_tool".into(), "old_input".into())];
    agent.pending_plan_exit_tool_id = Some("old-plan-tool".into());
    agent.inspection_progress.source_reads = 4;
    agent.last_query_plan_updated = true;
    agent.runtime_turn_id = Some("old-runtime-turn".into());
    agent.cancellation_token = Some(Arc::new(AtomicBool::new(true)));
    agent.plugin_session_start_hooks_ran = true;
    let lease = agent.begin_inference_turn();
    drop(lease);
    assert!(agent.inference_context.is_some());
    let hooks = Arc::new(HookRuntime::new(Arc::new(RuntimeEventBus::new(8))));
    hooks.push_output("old hook output".into());
    agent.hook_runtime = Some(hooks.clone());
    agent.reset_for_new_thread("new-thread".into());
    assert_eq!(agent.session_id, "new-thread");
    assert!(agent.stable_tool_schemas.is_none());
    assert!(agent.recent_tool_calls.is_empty());
    assert!(agent.pending_plan_exit_tool_id.is_none());
    assert_eq!(agent.inspection_progress.source_reads, 0);
    assert!(!agent.last_query_plan_updated);
    assert!(agent.runtime_turn_id.is_none());
    assert!(agent.cancellation_token.is_none());
    assert!(!agent.plugin_session_start_hooks_ran);
    assert!(agent.inference_context.is_none());
    assert!(hooks.blocking_drain_outputs().is_empty());
    assert!(Arc::ptr_eq(agent.hook_runtime.as_ref().unwrap(), &hooks));
}
