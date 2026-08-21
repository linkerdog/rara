use std::sync::Arc;

use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::support::test_runtime_storage;
use crate::agent::{Agent, Message};
use crate::llm::MockLlm;
use crate::tools::agent::AgentTreeControl;

#[test]
fn mailbox_delivery_is_ordered_after_history_and_checkpointed_once() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager.clone(),
        workspace,
    );
    let tool_use = Message {
        role: "assistant".to_string(),
        content: json!([{
            "type": "tool_use",
            "id": "tool-1",
            "name": "stub_tool",
            "input": {}
        }]),
    };
    let tool_result = Message {
        role: "user".to_string(),
        content: json!([{
            "type": "tool_result",
            "tool_use_id": "tool-1",
            "content": "ok"
        }]),
    };
    agent.history = vec![tool_use.clone(), tool_result.clone()];
    let control = Arc::new(AgentTreeControl::default());
    control
        .enqueue_test_message(
            &agent.session_id,
            Some("worker-1"),
            "completion",
            r#"{"status":"done"}"#,
        )
        .expect("enqueue mailbox message");
    agent.set_agent_tree_control(Some(control));

    assert_eq!(agent.inject_agent_mailbox_messages().expect("inject"), 1);
    assert_eq!(agent.history[0], tool_use);
    assert_eq!(agent.history[1], tool_result);
    assert_eq!(agent.history[2].role, "system");
    assert!(
        agent.history[2]
            .content
            .as_str()
            .expect("mailbox system content")
            .contains("worker-1")
    );
    assert_eq!(agent.inject_agent_mailbox_messages().expect("reinject"), 0);
    assert_eq!(agent.history.len(), 3);

    let persisted = session_manager
        .load_thread_history(&agent.session_id)
        .expect("persisted history");
    assert_eq!(persisted, agent.history);
}
