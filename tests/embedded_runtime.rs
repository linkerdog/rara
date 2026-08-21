use std::num::NonZeroUsize;
use std::sync::Arc;

use rara::{
    AgentEvent, AgentOutputMode, AgentTreeConfig, EmbeddedRuntime, EmbeddedRuntimeOptions,
    RaraConfig,
};
use tempfile::tempdir;

#[tokio::test]
async fn embedded_runtime_is_workspace_scoped_and_emits_typed_events() {
    let temp = tempdir().expect("tempdir");
    let workspace = temp.path().join("workspace");
    let state_root = temp.path().join("state");
    std::fs::create_dir_all(&workspace).expect("workspace");
    let current_dir = std::env::current_dir().expect("current dir");
    let config = RaraConfig {
        provider: "mock".to_string(),
        ..RaraConfig::default()
    };
    let options = EmbeddedRuntimeOptions {
        state_root: Some(state_root),
        agent_tree_config: AgentTreeConfig::new(NonZeroUsize::new(2).expect("positive capacity")),
        ..EmbeddedRuntimeOptions::default()
    };

    let mut runtime =
        EmbeddedRuntime::from_config_with_options(&config, &workspace, options.clone())
            .await
            .expect("embedded runtime");
    let second = EmbeddedRuntime::from_config_with_options(&config, &workspace, options)
        .await
        .expect("second embedded runtime");

    assert_eq!(runtime.workspace_root(), workspace);
    assert_eq!(std::env::current_dir().expect("current dir"), current_dir);
    assert_ne!(runtime.session_id(), second.session_id());
    assert!(!Arc::ptr_eq(
        &runtime.agent_tree_control(),
        &second.agent_tree_control()
    ));
    assert_eq!(runtime.agent_tree_control().max_active_subagents(), 2);
    assert!(runtime.list_agents().expect("agent snapshots").is_empty());

    let mut events = Vec::new();
    runtime
        .query_with_events("hello", AgentOutputMode::Silent, |event| events.push(event))
        .await
        .expect("query");
    assert!(events.iter().any(
        |event| matches!(event, AgentEvent::AssistantText(text) if text.contains("Mock Response"))
    ));
}
