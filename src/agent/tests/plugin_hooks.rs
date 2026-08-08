use std::fs;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::Result;
use async_trait::async_trait;
use rara_memory::memory_handle::MemoryHandle;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError, ToolManager};
use serde_json::{Value, json};

use super::support::{SequencedBackend, test_runtime_storage};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::hooks::{HookRegistry, HookSandbox};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, TokenUsage};

struct CancelledBackend;

#[async_trait]
impl LlmBackend for CancelledBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[Value],
    ) -> Result<LlmResponse> {
        anyhow::bail!("cancelled by user")
    }
    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> Result<String> {
        Ok("summary".to_string())
    }
}

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
        &crate::config::BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
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

#[tokio::test]
async fn plugin_session_end_runs_once_with_last_assistant_message() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    write_session_end_plugin(temp.path());

    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "final answer".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let hook_runtime = Arc::new(crate::hook_runtime::HookRuntime::new(Arc::new(
        crate::runtime_event_bus::RuntimeEventBus::new(4),
    )));
    let plugin_hooks = crate::plugin_middleware::register_plugin_hooks(
        &hook_runtime,
        None,
        temp.path(),
        &[],
        &crate::config::BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.history.push(crate::agent::Message {
        role: "user".to_string(),
        content: json!("complete"),
    });
    agent.set_hook_context(
        Arc::new(HookRegistry::new()),
        HookSandbox {
            workspace_root: temp.path().to_path_buf(),
            ..HookSandbox::default()
        },
        hook_runtime,
    );
    agent.set_plugin_hook_runtime(plugin_hooks);

    agent
        .run_agent_loop_with_limit(AgentOutputMode::Silent, &mut |_| {}, &mut 0)
        .await
        .expect("agent loop");

    let plugin_root = temp
        .path()
        .join(".rara")
        .join("plugins")
        .join("session-end");
    let hook_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join("session-end-input.json")).expect("hook input"),
    )
    .expect("valid hook input");
    assert_eq!(
        hook_input["hook_event"],
        serde_json::Value::String("SessionEnd".to_string())
    );
    assert_eq!(hook_input["tool_name"], serde_json::Value::Null);
    assert_eq!(hook_input["tool_input"], serde_json::Value::Null);
    assert_eq!(hook_input["tool_response"], serde_json::Value::Null);
    assert_eq!(
        hook_input["last_assistant_message"],
        serde_json::Value::String("final answer".to_string())
    );
    assert_eq!(hook_input["is_interrupt"], serde_json::Value::Bool(false));
    assert_eq!(
        fs::read_to_string(plugin_root.join("session-end-count")).expect("count"),
        "x"
    );
}

#[tokio::test]
async fn plugin_session_end_marks_cancelled_model_turn_as_interrupt() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    write_session_end_plugin(temp.path());

    let hook_runtime = Arc::new(crate::hook_runtime::HookRuntime::new(Arc::new(
        crate::runtime_event_bus::RuntimeEventBus::new(4),
    )));
    let plugin_hooks = crate::plugin_middleware::register_plugin_hooks(
        &hook_runtime,
        None,
        temp.path(),
        &[],
        &crate::config::BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(CancelledBackend),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.history.push(crate::agent::Message {
        role: "user".to_string(),
        content: json!("complete"),
    });
    agent.set_hook_context(
        Arc::new(HookRegistry::new()),
        HookSandbox {
            workspace_root: temp.path().to_path_buf(),
            ..HookSandbox::default()
        },
        hook_runtime,
    );
    agent.set_plugin_hook_runtime(plugin_hooks);

    let error = agent
        .run_agent_loop_with_limit(AgentOutputMode::Silent, &mut |_| {}, &mut 0)
        .await
        .expect_err("agent loop should return cancellation");

    assert!(error.to_string().contains("cancelled by user"));
    let plugin_root = temp
        .path()
        .join(".rara")
        .join("plugins")
        .join("session-end");
    let hook_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join("session-end-input.json")).expect("hook input"),
    )
    .expect("valid hook input");
    assert_eq!(
        hook_input["hook_event"],
        serde_json::Value::String("SessionEnd".to_string())
    );
    assert_eq!(hook_input["is_interrupt"], serde_json::Value::Bool(true));
    assert_eq!(
        hook_input["last_assistant_message"],
        serde_json::Value::Null
    );
    assert_eq!(
        fs::read_to_string(plugin_root.join("session-end-count")).expect("count"),
        "x"
    );
}

#[tokio::test]
async fn plugin_non_tool_lifecycle_hooks_run_from_agent_query() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    write_non_tool_lifecycle_plugin(temp.path());

    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "first answer".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "second answer".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let hook_runtime = Arc::new(crate::hook_runtime::HookRuntime::new(Arc::new(
        crate::runtime_event_bus::RuntimeEventBus::new(4),
    )));
    let plugin_hooks = crate::plugin_middleware::register_plugin_hooks(
        &hook_runtime,
        None,
        temp.path(),
        &[],
        &crate::config::BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
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

    agent
        .query_with_mode_and_events("first prompt".to_string(), AgentOutputMode::Silent, |_| {})
        .await
        .expect("first query");
    agent
        .query_with_mode_and_events("second prompt".to_string(), AgentOutputMode::Silent, |_| {})
        .await
        .expect("second query");

    let plugin_root = temp.path().join(".rara").join("plugins").join("lifecycle");
    assert_eq!(
        fs::read_to_string(plugin_root.join("session-start-count")).expect("session count"),
        "x"
    );
    assert_eq!(
        fs::read_to_string(plugin_root.join("prompt-submit-count")).expect("prompt count"),
        "xx"
    );
    let session_start_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join("session-start-input.json")).expect("session input"),
    )
    .expect("valid session input");
    assert_eq!(
        session_start_input["hook_event"],
        serde_json::Value::String("SessionStart".to_string())
    );
    assert_eq!(session_start_input["prompt"], serde_json::Value::Null);

    let prompt_submit_input: serde_json::Value = serde_json::from_str(
        &fs::read_to_string(plugin_root.join("prompt-submit-input.json")).expect("prompt input"),
    )
    .expect("valid prompt input");
    assert_eq!(
        prompt_submit_input["hook_event"],
        serde_json::Value::String("UserPromptSubmit".to_string())
    );
    assert_eq!(
        prompt_submit_input["prompt"],
        serde_json::Value::String("second prompt".to_string())
    );
    assert_eq!(prompt_submit_input["tool_name"], serde_json::Value::Null);
}

#[tokio::test]
async fn plugin_session_start_waits_until_plugin_runtime_is_attached() {
    let (temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    write_non_tool_lifecycle_plugin(temp.path());

    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "first answer".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "second answer".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let hook_runtime = Arc::new(crate::hook_runtime::HookRuntime::new(Arc::new(
        crate::runtime_event_bus::RuntimeEventBus::new(4),
    )));
    let plugin_hooks = crate::plugin_middleware::register_plugin_hooks(
        &hook_runtime,
        None,
        temp.path(),
        &[],
        &crate::config::BuiltinPluginConfig::default(),
        "session-1",
    )
    .await;
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
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

    agent
        .query_with_mode_and_events("first prompt".to_string(), AgentOutputMode::Silent, |_| {})
        .await
        .expect("first query");
    agent.set_plugin_hook_runtime(plugin_hooks);
    agent
        .query_with_mode_and_events("second prompt".to_string(), AgentOutputMode::Silent, |_| {})
        .await
        .expect("second query");

    let plugin_root = temp.path().join(".rara").join("plugins").join("lifecycle");
    assert_eq!(
        fs::read_to_string(plugin_root.join("session-start-count")).expect("session count"),
        "x"
    );
    assert_eq!(
        fs::read_to_string(plugin_root.join("prompt-submit-count")).expect("prompt count"),
        "x"
    );
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

fn write_non_tool_lifecycle_plugin(workspace_root: &std::path::Path) {
    let root = workspace_root
        .join(".rara")
        .join("plugins")
        .join("lifecycle");
    fs::create_dir_all(root.join(".claude-plugin")).expect("metadata dir");
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        json!({
            "name": "lifecycle",
            "version": "1.0.0",
            "description": "test lifecycle hooks"
        })
        .to_string(),
    )
    .expect("plugin json");
    fs::create_dir_all(root.join("hooks")).expect("hooks dir");
    fs::write(
        root.join("hooks").join("hooks.json"),
        json!({
            "SessionStart": [{
                "hooks": [{
                    "type": "command",
                    "command": "cat > \"$CLAUDE_PLUGIN_ROOT/session-start-input.json\"; printf x >> \"$CLAUDE_PLUGIN_ROOT/session-start-count\"",
                    "timeout": 5
                }]
            }],
            "UserPromptSubmit": [{
                "hooks": [{
                    "type": "command",
                    "command": "cat > \"$CLAUDE_PLUGIN_ROOT/prompt-submit-input.json\"; printf x >> \"$CLAUDE_PLUGIN_ROOT/prompt-submit-count\"",
                    "timeout": 5
                }]
            }]
        })
        .to_string(),
    )
    .expect("hooks json");
}

fn write_session_end_plugin(workspace_root: &std::path::Path) {
    let root = workspace_root
        .join(".rara")
        .join("plugins")
        .join("session-end");
    fs::create_dir_all(root.join(".claude-plugin")).expect("metadata dir");
    fs::write(
        root.join(".claude-plugin").join("plugin.json"),
        json!({
            "name": "session-end",
            "version": "1.0.0",
            "description": "test session end"
        })
        .to_string(),
    )
    .expect("plugin json");
    fs::create_dir_all(root.join("hooks")).expect("hooks dir");
    fs::write(
        root.join("hooks").join("hooks.json"),
        json!({
            "SessionEnd": [{
                "hooks": [{
                    "type": "command",
                    "command": "cat > \"$CLAUDE_PLUGIN_ROOT/session-end-input.json\"; printf x >> \"$CLAUDE_PLUGIN_ROOT/session-end-count\"",
                    "timeout": 5
                }]
            }]
        })
        .to_string(),
    )
    .expect("hooks json");
}
