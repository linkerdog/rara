use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

use clap::Parser;
use rara_tools::tool::{Tool, ToolError, ToolManager};
use serde_json::{Value, json};

use super::{Cli, StartupPermissions, apply_cli_overrides};
use crate::config::RaraConfig;

#[test]
fn startup_permission_flag_maps_to_explicit_override_only() {
    let default = Cli::try_parse_from(["rara"]).unwrap();
    assert_eq!(
        StartupPermissions::from_cli(&default)
            .unwrap()
            .tui_override(),
        None
    );
    for args in [
        vec!["rara", "--dangerously-skip-permissions"],
        vec!["rara", "tui", "--dangerously-skip-permissions"],
        vec!["rara", "--dangerously-skip-permissions", "resume", "--last"],
        vec!["rara", "ask", "hello", "--dangerously-skip-permissions"],
        vec!["rara", "print", "hello", "--dangerously-skip-permissions"],
        vec!["rara", "wire", "hello", "--dangerously-skip-permissions"],
        vec![
            "rara",
            "exec",
            "--full-access",
            "--dangerously-skip-permissions",
            "hello",
        ],
    ] {
        let cli = Cli::try_parse_from(&args).unwrap();
        assert_eq!(
            StartupPermissions::from_cli(&cli).unwrap().tui_override(),
            Some(crate::tui::state::PermissionMode::FullAccess),
            "{args:?}"
        );
    }
}

#[test]
fn bypass_does_not_modify_saved_configuration() {
    let mut normal = RaraConfig::default();
    let mut bypass = normal.clone();
    apply_cli_overrides(&mut normal, Cli::try_parse_from(["rara"]).unwrap());
    apply_cli_overrides(
        &mut bypass,
        Cli::try_parse_from(["rara", "--dangerously-skip-permissions"]).unwrap(),
    );
    assert_eq!(
        serde_json::to_value(normal).unwrap(),
        serde_json::to_value(bypass).unwrap()
    );
}

#[test]
fn bypass_rejects_unsupported_authorization_surfaces() {
    for command in ["acp", "threads", "login"] {
        let cli = Cli::try_parse_from(["rara", "--dangerously-skip-permissions", command]).unwrap();
        assert!(StartupPermissions::from_cli(&cli).is_err(), "{command}");
    }
}

struct ShellBackend(AtomicUsize);

#[async_trait::async_trait]
impl crate::llm::LlmBackend for ShellBackend {
    async fn ask(
        &self,
        _messages: &[crate::agent::Message],
        _tools: &[Value],
    ) -> anyhow::Result<crate::llm::LlmResponse> {
        let content = if self.0.fetch_add(1, Ordering::Relaxed) == 0 {
            crate::llm::ContentBlock::ToolUse {
                id: "shell-1".into(),
                name: "bash".into(),
                input: json!({"command": "touch permission-marker", "sandbox_permissions": "require_escalated"}),
            }
        } else {
            crate::llm::ContentBlock::Text {
                text: "Done.".into(),
            }
        };
        Ok(crate::llm::LlmResponse {
            content: vec![content],
            stop_reason: None,
            usage: None,
        })
    }
    async fn summarize(
        &self,
        _messages: &[crate::agent::Message],
        _instruction: &str,
    ) -> anyhow::Result<String> {
        Ok("Summary".into())
    }
}

struct RecordedShell(Arc<AtomicBool>);

#[async_trait::async_trait]
impl Tool for RecordedShell {
    fn name(&self) -> &str {
        "bash"
    }
    fn description(&self) -> &str {
        "Record a shell invocation without running a process."
    }
    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        self.0.store(true, Ordering::Relaxed);
        Ok(json!({"output": "finished with exit code 0"}))
    }
}

#[tokio::test]
async fn headless_startup_bypass_reaches_actual_tool_authorization() {
    for mode in [StartupPermissions::Default, StartupPermissions::FullAccess] {
        let temp = tempfile::tempdir().unwrap();
        let workspace = temp.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let mut config = RaraConfig {
            provider: "mock".into(),
            ..Default::default()
        };
        config.sandbox_workspace_write.network_access = false;
        config.builtin_plugins.nowledge_mem.enabled = false;
        let invoked = Arc::new(AtomicBool::new(false));
        let mut tools = ToolManager::new();
        tools.register(Box::new(RecordedShell(invoked.clone())));
        let options = crate::runtime_context::RuntimeBootstrapOptions::default()
            .with_rara_home(Some(temp.path().join("home")))
            .with_backend(Some(Arc::new(ShellBackend(AtomicUsize::new(0)))))
            .with_tool_manager(Some(tools))
            .with_extension_discovery(false)
            .with_memory_facilities(false)
            .with_transcript_persistence(false);
        let bootstrap = crate::runtime_context::initialize_rara_context_for_workspace_with_options(
            &config,
            Some(&workspace),
            None,
            options,
        )
        .await
        .unwrap();
        let network = bootstrap.sandbox_network_access.clone();
        let session = mode.start_headless_session(bootstrap).await.unwrap();
        let mut approvals = 0;
        let result = session
            .query_with_events(
                "Run the requested command.",
                crate::agent::AgentOutputMode::Silent,
                |event| {
                    if matches!(event, crate::agent::AgentEvent::ApprovalRequested { .. }) {
                        approvals += 1;
                    }
                },
            )
            .await;
        session.shutdown().await.unwrap();
        result.unwrap();
        assert_eq!(
            invoked.load(Ordering::Relaxed),
            mode == StartupPermissions::FullAccess
        );
        assert_eq!(
            network.load(Ordering::Relaxed),
            mode == StartupPermissions::FullAccess
        );
        assert_eq!(
            approvals,
            if mode == StartupPermissions::FullAccess {
                0
            } else {
                1
            }
        );
    }
}
