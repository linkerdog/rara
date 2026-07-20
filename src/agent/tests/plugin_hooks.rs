use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use rara_memory::vectordb::VectorDB;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError, ToolManager};
use serde_json::{Value, json};

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::hooks::{HookRegistry, HookSandbox};
use crate::llm::{ContentBlock, LlmResponse, TokenUsage};

struct CountingTool {
    calls: Arc<AtomicUsize>,
}

#[tool_spec(
    name = "stub_tool",
    description = "Return a simple structured result",
    input_schema = { "type": "object" }
)]
#[async_trait]
impl Tool for CountingTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        Ok(json!({ "status": "ok" }))
    }
}

#[tokio::test]
async fn plugin_pre_tool_use_continue_false_blocks_tool_execution() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    write_blocking_plugin(temp.path());

    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "stub_tool".to_string(),
                input: json!({ "path": "src/lib.rs" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "blocked".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let calls = Arc::new(AtomicUsize::new(0));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(CountingTool {
        calls: calls.clone(),
    }));
    let hook_runtime = Arc::new(crate::hook_runtime::HookRuntime::new(Arc::new(
        crate::runtime_event_bus::RuntimeEventBus::new(4),
    )));
    let plugin_hooks = crate::plugin_middleware::register_plugin_hooks(
        &hook_runtime,
        None,
        temp.path(),
        &[],
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(VectorDB::new(&rara_dir.join("lancedb").to_string_lossy())),
        session_manager,
        workspace,
    );
    agent.set_hook_context(
        Arc::new(HookRegistry::new()),
        HookSandbox {
            workspace_root: temp.path().to_path_buf(),
            ..HookSandbox::default()
        },
        hook_runtime,
    );
    agent.set_plugin_hook_runtime(plugin_hooks);
    let mut events = Vec::new();

    agent
        .query_with_mode_and_events("run tool".to_string(), AgentOutputMode::Silent, |event| {
            events.push(event)
        })
        .await
        .expect("query");

    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            name,
            content,
            is_error: true
        } if name == "stub_tool" && content.contains("blocked by policy")
    )));
    assert!(backend.observed_messages()[1].iter().any(|message| {
        message.role == "user" && message.content.to_string().contains("blocked by policy")
    }));
}

fn write_blocking_plugin(workspace_root: &std::path::Path) {
    let root = workspace_root.join(".rara").join("plugins").join("blocker");
    fs::create_dir_all(root.join(".claude-plugin")).expect("metadata dir");
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        json!({
            "name": "blocker",
            "version": "1.0.0",
            "description": "test blocker"
        })
        .to_string(),
    )
    .expect("plugin json");
    fs::create_dir_all(root.join("hooks")).expect("hooks dir");
    fs::write(
        root.join("hooks").join("hooks.json"),
        json!({
            "PreToolUse": [{
                "matcher": "stub_tool",
                "hooks": [{
                    "type": "command",
                    "command": "cat > hook-input.json; echo '{\"continue\":false,\"reason\":\"blocked by policy\"}'",
                    "timeout": 5
                }]
            }]
        })
        .to_string(),
    )
    .expect("hooks json");
}
