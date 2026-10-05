use std::sync::{Arc, Mutex};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rara_state::state_db::StateDb;
use rara_tools::tool::ToolManager;
use serde_json::json;

use crate::agent::{AgentExecutionMode, Message, PendingUserInput, PlanStep, PlanStepStatus};
use crate::config::{ConfigManager, RaraConfig};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse};
use crate::oauth::OAuthManager;
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::runtime_goals::RalphGoal;
use crate::thread_store::{ThreadRecorder, ThreadStore};
use crate::tui::keymap::map_key_to_event;
use crate::tui::message_role::MessageRole;
use crate::tui::runtime::RuntimeCommandProcessor;
use crate::tui::state::{TaskKind, TuiApp};
use crate::tui::testing::FakeRuntimeClient;

#[derive(Default)]
struct Backend(Mutex<Vec<Vec<Message>>>);

#[async_trait::async_trait]
impl LlmBackend for Backend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        self.0.lock().unwrap().push(messages.to_vec());
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Done.".into(),
            }],
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }
    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".into())
    }
}

struct Fixture {
    dir: tempfile::TempDir,
    app: TuiApp,
    processor: RuntimeCommandProcessor,
    backend: Arc<Backend>,
    db: Arc<StateDb>,
    oauth: Arc<OAuthManager>,
}

impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let backend = Arc::new(Backend::default());
        let mut config = RaraConfig::default();
        config.builtin_plugins.nowledge_mem.enabled = false;
        let options = RuntimeBootstrapOptions::with_plugin_dirs(Vec::new())
            .with_rara_home(Some(dir.path().join("state")))
            .with_backend(Some(backend.clone()))
            .with_tool_manager(Some(ToolManager::new()))
            .with_extension_discovery(false)
            .with_memory_facilities(false)
            .with_transcript_persistence(false);
        let bootstrap = initialize_rara_context_for_workspace_with_options(
            &config,
            Some(dir.path()),
            None,
            options,
        )
        .await
        .unwrap();
        let runtime = RuntimeClient::from_bootstrap(bootstrap).await;
        let root = runtime
            .agent()
            .unwrap()
            .session_manager
            .storage_dir
            .parent()
            .unwrap()
            .to_path_buf();
        let db = Arc::new(StateDb::new_for_root_dir(root).unwrap());
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.config = config;
        app.event_bus = Some(runtime.event_bus.clone());
        app.mcp_manager = Some(runtime.mcp_manager.clone());
        app.memory_handler = Some(Arc::new(
            crate::protocol_sources::MemoryControlHandler::new(runtime.event_bus.clone()),
        ));
        app.goal_handle = runtime.goal_handle.clone();
        let mut processor = RuntimeCommandProcessor::new(runtime);
        processor.sync_snapshot(&mut app);
        app.attach_state_db(db.clone());
        let oauth = Arc::new(OAuthManager::new_for_config_dir(dir.path().join("oauth")).unwrap());
        Self {
            dir,
            app,
            processor,
            backend,
            db,
            oauth,
        }
    }

    async fn enter(&mut self, text: &str) {
        self.app.bottom_pane.input = text.into();
        self.app.bottom_pane.input_cursor_offset = None;
        self.app.sync_command_palette_with_input();
        let port = FakeRuntimeClient::new(Default::default());
        let event = map_key_to_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE), &self.app);
        self.processor
            .dispatch_event(&mut self.app, event, &self.oauth, &port)
            .await
            .unwrap();
        for command in port.commands() {
            self.processor
                .apply_command(&mut self.app, command)
                .await
                .unwrap();
        }
    }

    async fn finish(&mut self) {
        let task = self
            .app
            .bottom_pane
            .running_task
            .as_mut()
            .expect("running task");
        let result = tokio::time::timeout(Duration::from_secs(10), &mut task.handle)
            .await
            .unwrap();
        self.processor
            .complete(&mut self.app, Box::new(result))
            .await
            .unwrap();
    }

    async fn flush(&mut self) {
        self.app.storage.as_ref().unwrap().flush().await.unwrap();
    }
    async fn shutdown(&mut self) {
        self.app.storage.as_mut().unwrap().shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn new_preserves_config_and_old_durable_state_but_clears_thread_state() {
    let mut f = Fixture::new().await;
    let old_id = f.processor.session_id().unwrap();
    let agent = f.processor.agent_mut().as_mut().unwrap();
    agent.history.push(Message {
        role: "user".into(),
        content: json!("old request"),
    });
    ThreadRecorder::new(&f.db)
        .persist_history_checkpoint(&old_id, &agent.history)
        .unwrap();
    agent.total_input_tokens = 321;
    agent.aux_total_cache_hit_tokens = 123;
    agent.token_budget_exhausted = true;
    agent.compact_state.compaction_count = 4;
    agent.compact_state.context_window_tokens = Some(131072);
    agent.compact_state.compact_threshold_tokens = 100000;
    agent.compact_state.reserved_output_tokens = 8192;
    agent.current_plan.push(PlanStep {
        step: "old step".into(),
        status: PlanStepStatus::InProgress,
    });
    agent.plan_explanation = Some("old plan".into());
    agent.pending_user_input = Some(PendingUserInput {
        question: "old question".into(),
        options: Vec::new(),
        note: None,
    });
    agent.task_list_id = "shared-list".into();
    agent.execution_mode = AgentExecutionMode::Plan;
    let mut prompt = agent.prompt_config().clone();
    prompt.append_system_prompt = Some("workspace policy".into());
    agent.set_prompt_config(prompt);
    let backend = agent.llm_backend.clone();
    f.processor.sync_snapshot(&mut f.app);
    f.app
        .goal_handle
        .replace(Some(RalphGoal::new("old goal".into(), Some(999))))
        .unwrap();
    let ticket = f.app.goal_handle.resume_ticket().unwrap();
    f.app.push_entry(MessageRole::User, "old visible request");
    f.app.push_entry(MessageRole::Agent, "old answer");
    f.enter("/rename original thread").await;
    f.finish().await;
    f.enter("/new").await;
    assert_eq!(f.processor.session_id().as_deref(), Some(old_id.as_str()));
    f.app.bottom_pane.input = "draft typed while preparing".into();
    f.app.snapshot.extension_hook_count = 7;
    f.finish().await;
    let agent = f.processor.agent().unwrap();
    assert_ne!(agent.session_id, old_id);
    assert!(agent.history.is_empty());
    assert!(agent.current_plan.is_empty());
    assert!(agent.pending_user_input.is_none());
    assert_eq!(agent.total_input_tokens, 0);
    assert_eq!(agent.aux_total_cache_hit_tokens, 0);
    assert!(!agent.token_budget_exhausted);
    assert_eq!(agent.compact_state.compaction_count, 0);
    assert_eq!(agent.compact_state.context_window_tokens, Some(131072));
    assert_eq!(agent.compact_state.compact_threshold_tokens, 100000);
    assert_eq!(agent.compact_state.reserved_output_tokens, 8192);
    assert_eq!(agent.task_list_id, "shared-list");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(Arc::ptr_eq(&agent.llm_backend, &backend));
    assert_eq!(
        agent.prompt_config().append_system_prompt.as_deref(),
        Some("workspace policy")
    );
    assert!(f.app.goal_handle.snapshot().is_none());
    assert_eq!(f.app.snapshot.extension_hook_count, 7);
    assert!(!f.app.goal_handle.matches_resume_ticket(&ticket));
    assert!(f.app.snapshot.pending_interactions.is_empty());
    assert_eq!(f.app.bottom_pane.input, "draft typed while preparing");
    assert!(
        !f.app
            .active_turn
            .entries
            .iter()
            .any(|e| e.message.contains("old answer"))
    );
    f.flush().await;
    let store = ThreadStore::new(&f.processor.agent().unwrap().session_manager, &f.db);
    let old = store.load_thread(&old_id).unwrap();
    assert_eq!(old.metadata.title.as_deref(), Some("original thread"));
    assert_eq!(old.plan_steps.len(), 1);
    assert!(
        old.history
            .iter()
            .any(|m| m.content == json!("old request"))
    );
    assert!(f.db.try_load_goal(&old_id).unwrap().is_some());
    let new = store
        .load_thread(&f.processor.session_id().unwrap())
        .unwrap();
    assert!(new.metadata.title.is_none());
    assert_eq!(new.metadata.agent_mode, "plan");
    assert!(new.history.is_empty());
    f.app.insert_resume_search_text("original thread");
    f.app.finish_resume_query_for_test().await;
    assert!(
        f.app
            .recent_threads
            .iter()
            .any(|thread| thread.metadata.session_id == old_id)
    );
    assert!(
        f.app
            .recent_threads
            .iter()
            .all(|thread| thread.metadata.title.as_deref() == Some("original thread"))
    );
    f.shutdown().await;
}

#[tokio::test]
async fn failed_old_thread_flush_does_not_switch_identity_or_erase_history() {
    let mut f = Fixture::new().await;
    let old_id = f.processor.session_id().unwrap();
    f.flush().await;
    let bad = rara_persistence::thread_turn_log::turn_log_path(&f.db.rollout_root(), &old_id);
    std::fs::create_dir(&bad).unwrap();
    f.app.push_entry(MessageRole::User, "must survive");
    f.enter("/new").await;
    f.finish().await;
    assert_eq!(f.processor.session_id().as_deref(), Some(old_id.as_str()));
    assert!(f.app.notice_text().unwrap().contains("failed"));
    assert!(
        f.app
            .committed_turns
            .iter()
            .flat_map(|t| &t.entries)
            .any(|e| e.message == "must survive")
    );
    std::fs::remove_dir(bad).unwrap();
    f.shutdown().await;
}

#[tokio::test]
async fn prompt_queued_during_new_runs_in_the_new_thread() {
    let mut f = Fixture::new().await;
    let old = f.processor.session_id().unwrap();
    f.enter("/new").await;
    f.enter("new thread request").await;
    assert!(matches!(
        f.app.bottom_pane.running_task.as_ref().unwrap().kind,
        TaskKind::ThreadCommand
    ));
    f.finish().await;
    assert!(matches!(
        f.app.bottom_pane.running_task.as_ref().unwrap().kind,
        TaskKind::Query
    ));
    f.finish().await;
    assert_ne!(f.processor.session_id().as_deref(), Some(old.as_str()));
    assert!(
        f.backend
            .0
            .lock()
            .unwrap()
            .iter()
            .flatten()
            .any(|m| m.content.to_string().contains("new thread request"))
    );
    f.shutdown().await;
}

#[tokio::test]
async fn export_preserves_cleared_turns_and_refuses_overwrite() {
    let mut f = Fixture::new().await;
    f.app.push_entry(MessageRole::User, "before clear");
    f.app.push_entry(MessageRole::Agent, "earlier answer");
    f.enter("/clear").await;
    f.app.push_entry(MessageRole::User, "after clear");
    f.app.push_entry(MessageRole::Agent, "later answer");
    f.enter("/export conversation.md").await;
    f.finish().await;
    let md = std::fs::read_to_string(f.dir.path().join("conversation.md")).unwrap();
    for expected in [
        "before clear",
        "earlier answer",
        "after clear",
        "later answer",
    ] {
        assert!(md.contains(expected), "missing {expected}: {md}");
    }
    assert!(md.find("earlier answer") < md.find("later answer"));
    f.enter("/export conversation.json").await;
    f.finish().await;
    let json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(f.dir.path().join("conversation.json")).unwrap())
            .unwrap();
    assert_eq!(json["schema_version"], 1);
    assert!(
        json["messages"]
            .as_array()
            .unwrap()
            .iter()
            .any(|m| m["content"] == "earlier answer")
    );
    f.enter("/export conversation.md").await;
    f.finish().await;
    assert!(f.app.notice_text().unwrap().contains("without overwriting"));
    assert_eq!(
        std::fs::read_to_string(f.dir.path().join("conversation.md")).unwrap(),
        md
    );
    f.enter("/export missing/child.md").await;
    f.finish().await;
    assert!(
        f.app
            .notice_text()
            .unwrap()
            .contains("directory does not exist")
    );
    f.shutdown().await;
}

#[tokio::test]
async fn init_submits_an_agent_prompt_instead_of_writing_instructions_directly() {
    let mut f = Fixture::new().await;
    f.enter("/init").await;
    f.finish().await;
    let observed = f
        .backend
        .0
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .map(|m| m.content.to_string())
        .collect::<String>();
    assert!(observed.contains("AGENTS.md"));
    assert!(observed.contains("preserve applicable guidance"));
    assert!(observed.contains("normal editing and approval flow"));
    assert!(!f.dir.path().join("AGENTS.md").exists());
    f.shutdown().await;
}

#[tokio::test]
async fn usage_errors_and_busy_thread_commands_do_not_start_competing_tasks() {
    let mut f = Fixture::new().await;
    for command in ["/new extra", "/init extra", "/diff extra", "/rename"] {
        f.enter(command).await;
        assert!(!f.app.is_busy(), "{command}");
        assert!(f.app.notice_text().unwrap().contains("Usage:"), "{command}");
    }
    f.enter("/export wrong.txt").await;
    f.finish().await;
    assert!(f.app.notice_text().unwrap().contains("Usage:"));
    assert!(!f.dir.path().join("wrong.txt").exists());
    f.enter("/new").await;
    for command in ["/new", "/rename blocked", "/init", "/export blocked.md"] {
        f.enter(command).await;
        assert!(
            f.app.notice_text().unwrap().contains("Unavailable"),
            "{command}"
        );
        assert!(matches!(
            f.app.bottom_pane.running_task.as_ref().unwrap().kind,
            TaskKind::ThreadCommand
        ));
    }
    f.finish().await;
    assert!(f.backend.0.lock().unwrap().is_empty());
    f.shutdown().await;
}

#[tokio::test]
async fn export_filters_model_context_uses_unique_defaults_and_rejects_corruption() {
    let mut f = Fixture::new().await;
    let id = f.processor.session_id().unwrap();
    let mut history = vec![Message {
        role: "user".into(),
        content: json!([
            { "type": "rara_model_context", "kind": "retrieved_memory", "text": "model-only secret context" },
            { "type": "text", "text": "visible request" }
        ]),
    }];
    for role in ["system", "developer"] {
        history.push(Message {
            role: role.into(),
            content: json!("hidden policy context"),
        });
    }
    f.flush().await;
    ThreadRecorder::new(&f.db)
        .persist_history_checkpoint(&id, &history)
        .unwrap();
    let agent = f.processor.agent().unwrap();
    let store = ThreadStore::new(&agent.session_manager, &f.db);
    let first = store.export_thread_file(&id, None).unwrap();
    let second = store.export_thread_file(&id, None).unwrap();
    assert_ne!(first, second);
    let path = store.export_thread_file(&id, Some("context.json")).unwrap();
    for path in [first, second, path] {
        let text = std::fs::read_to_string(path).unwrap();
        assert!(text.contains("visible request"));
        assert!(!text.contains("model-only secret context"));
        assert!(!text.contains("rara_model_context"));
        assert!(!text.contains("hidden policy context"));
    }
    let turn_log = rara_persistence::thread_turn_log::turn_log_path(&f.db.rollout_root(), &id);
    std::fs::write(&turn_log, "{broken turn\n").unwrap();
    let error = store
        .export_thread_file(&id, Some("corrupt.md"))
        .unwrap_err();
    assert!(error.to_string().contains("line 1"));
    assert!(!f.dir.path().join("corrupt.md").exists());
    f.shutdown().await;
}
