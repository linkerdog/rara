use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rara_tools::tool::ToolManager;

use super::*;
use crate::config::{ConfigManager, RaraConfig};
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, Message};
use crate::runtime_client::RuntimeClient;
use crate::runtime_context::{
    RuntimeBootstrapOptions, initialize_rara_context_for_workspace_with_options,
};
use crate::tui::app_event::AppEvent;
use crate::tui::runtime::{QueryStopKind, RuntimeCommandProcessor};
use crate::tui::runtime_port::{RuntimeCommand, RuntimeMaintenanceCommand};
use crate::tui::testing::FakeRuntimeClient;

#[derive(Default)]
struct Backend {
    calls: AtomicUsize,
    hold: AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
    prompts: Mutex<Vec<String>>,
}

#[async_trait::async_trait]
impl LlmBackend for Backend {
    async fn ask(
        &self,
        messages: &[Message],
        _tools: &[serde_json::Value],
    ) -> anyhow::Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.prompts
            .lock()
            .unwrap()
            .extend(messages.iter().map(|m| m.content.to_string()));
        self.entered.notify_one();
        if self.hold.load(Ordering::SeqCst) {
            self.release.notified().await;
        }
        Ok(LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Reviewed changes.".into(),
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
    _dir: tempfile::TempDir,
    app: TuiApp,
    processor: RuntimeCommandProcessor,
    backend: Arc<Backend>,
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
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.config = config;
        app.event_bus = Some(runtime.event_bus.clone());
        app.goal_handle = runtime.goal_handle.clone();
        let mut processor = RuntimeCommandProcessor::new(runtime);
        processor.sync_snapshot(&mut app);
        Self {
            _dir: dir,
            app,
            processor,
            backend,
        }
    }

    async fn completion(&mut self) -> Result<TaskCompletion, tokio::task::JoinError> {
        tokio::time::timeout(
            Duration::from_secs(10),
            &mut self.app.bottom_pane.running_task.as_mut().unwrap().handle,
        )
        .await
        .unwrap()
    }

    async fn finish(&mut self) {
        let completion = self.completion().await;
        self.processor
            .complete(&mut self.app, Box::new(completion))
            .await
            .unwrap();
    }
}

struct Capture {
    entered: tokio::sync::Notify,
    result: Mutex<Option<tokio::sync::oneshot::Receiver<anyhow::Result<CapturedDiff>>>>,
}

#[async_trait::async_trait]
impl DiffCapture for Capture {
    async fn capture(&self, _cwd: &Path) -> anyhow::Result<CapturedDiff> {
        let result = self.result.lock().unwrap().take().unwrap();
        self.entered.notify_one();
        result.await?
    }
}

fn scripted_capture() -> (
    Arc<Capture>,
    tokio::sync::oneshot::Sender<anyhow::Result<CapturedDiff>>,
) {
    let (sender, result) = tokio::sync::oneshot::channel();
    (
        Arc::new(Capture {
            entered: tokio::sync::Notify::new(),
            result: Mutex::new(Some(result)),
        }),
        sender,
    )
}

#[tokio::test]
async fn pending_capture_keeps_input_responsive_and_clean_result_does_not_query() {
    let mut fixture = Fixture::new().await;
    let session = fixture.processor.session_id();
    let goal = crate::runtime_goals::RalphGoal::new("existing goal".into(), None);
    fixture.app.goal_handle.replace(Some(goal.clone())).unwrap();
    let (capture, result) = scripted_capture();
    start_capture(&mut fixture.app, capture.clone());
    capture.entered.notified().await;
    let oauth = Arc::new(
        crate::oauth::OAuthManager::new_for_config_dir(fixture._dir.path().join("oauth")).unwrap(),
    );
    fixture
        .processor
        .dispatch_event(
            &mut fixture.app,
            AppEvent::InputChar('x'),
            &oauth,
            &FakeRuntimeClient::new(Default::default()),
        )
        .await
        .unwrap();
    assert_eq!(fixture.app.bottom_pane.input, "x");
    assert_eq!(fixture.processor.session_id(), session);
    assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 0);
    result.send(Ok(CapturedDiff::default())).unwrap();
    fixture.finish().await;
    assert_eq!(fixture.app.runtime_phase, RuntimePhase::Idle);
    assert_eq!(
        fixture.app.bottom_pane.notice.as_deref(),
        Some("No staged or unstaged changes to review.")
    );
    assert_eq!(fixture.processor.session_id(), session);
    assert_eq!(fixture.app.goal_handle.snapshot(), Some(goal));
    assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 0);
    assert!(!fixture.app.is_busy());
}

#[tokio::test]
async fn failed_capture_preserves_agent_and_a_later_review_still_runs_once() {
    let mut fixture = Fixture::new().await;
    let session = fixture.processor.session_id();
    let (capture, result) = scripted_capture();
    start_capture(&mut fixture.app, capture);
    result
        .send(Err(anyhow::anyhow!("fatal: scripted invalid index")))
        .unwrap();
    fixture.finish().await;
    assert_eq!(fixture.app.runtime_phase, RuntimePhase::Failed);
    assert!(
        fixture
            .app
            .bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("scripted invalid index")
    );
    assert_eq!(fixture.processor.session_id(), session);
    assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 0);

    let (capture, result) = scripted_capture();
    start_capture(&mut fixture.app, capture);
    result
        .send(Ok(CapturedDiff {
            text: "+review me\n".repeat(700),
            truncated: true,
        }))
        .unwrap();
    fixture.finish().await;
    assert!(fixture.processor.agent().is_none());
    assert!(matches!(
        fixture.app.bottom_pane.running_task.as_ref().unwrap().kind,
        TaskKind::Query
    ));
    fixture.finish().await;
    assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 1);
    let prompts = fixture.backend.prompts.lock().unwrap().join("\n");
    assert!(prompts.contains("Full diff truncated"));
    assert_eq!(prompts.matches("+review me").count(), 600);
    assert_eq!(fixture.processor.session_id(), session);
}

#[tokio::test]
async fn cancellation_prevents_both_pending_and_already_completed_capture_from_starting_review() {
    for finished in [false, true] {
        let mut fixture = Fixture::new().await;
        let session = fixture.processor.session_id();
        let (capture, result) = scripted_capture();
        start_capture(&mut fixture.app, capture.clone());
        capture.entered.notified().await;
        let completion = if finished {
            result
                .send(Ok(CapturedDiff {
                    text: "+change".into(),
                    truncated: false,
                }))
                .unwrap();
            Some(fixture.completion().await)
        } else {
            None
        };
        assert!(super::super::tasks::request_running_task_cancellation(
            &mut fixture.app,
            QueryStopKind::Cancel
        ));
        let completion = match completion {
            Some(completion) => completion,
            None => fixture.completion().await,
        };
        fixture
            .processor
            .complete(&mut fixture.app, Box::new(completion))
            .await
            .unwrap();
        assert!(!fixture.app.is_busy());
        assert_eq!(fixture.app.runtime_phase, RuntimePhase::Idle);
        assert_eq!(
            fixture.app.bottom_pane.notice.as_deref(),
            Some("Review preparation cancelled.")
        );
        assert_eq!(fixture.processor.session_id(), session);
        assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 0);
    }
}

#[tokio::test]
async fn runtime_review_command_preserves_agent_and_prepared_query_exposes_cancellation() {
    let mut fixture = Fixture::new().await;
    fixture
        .processor
        .apply_command(
            &mut fixture.app,
            RuntimeCommand::Maintenance(RuntimeMaintenanceCommand::Review),
        )
        .await
        .unwrap();
    assert!(fixture.processor.agent().is_some());
    assert!(matches!(
        fixture.app.bottom_pane.running_task.as_ref().unwrap().kind,
        TaskKind::ReviewPreparation
    ));
    fixture.finish().await;
    assert_eq!(
        fixture.app.runtime_phase,
        RuntimePhase::Failed,
        "fixture is not a repository"
    );

    fixture.backend.hold.store(true, Ordering::SeqCst);
    let (capture, result) = scripted_capture();
    start_capture(&mut fixture.app, capture);
    result
        .send(Ok(CapturedDiff {
            text: "+change".into(),
            truncated: false,
        }))
        .unwrap();
    fixture.finish().await;
    fixture.backend.entered.notified().await;
    assert!(super::super::tasks::request_running_task_cancellation(
        &mut fixture.app,
        QueryStopKind::Cancel
    ));
    fixture.backend.release.notify_one();
    fixture.finish().await;
    assert!(!fixture.app.is_busy());
    assert!(fixture.processor.agent().is_some());
}

#[tokio::test]
async fn preparation_rejects_competing_maintenance_and_applies_pending_permissions() {
    let mut fixture = Fixture::new().await;
    let session = fixture.processor.session_id();
    let (capture, result) = scripted_capture();
    start_capture(&mut fixture.app, capture.clone());
    capture.entered.notified().await;
    let task_id = fixture
        .app
        .bottom_pane
        .running_task
        .as_ref()
        .unwrap()
        .handle
        .id();
    for command in [
        RuntimeMaintenanceCommand::Compact,
        RuntimeMaintenanceCommand::Rebuild,
        RuntimeMaintenanceCommand::RefreshModelCatalog(
            rara_provider_catalog::ModelCatalogProvider::DeepSeek,
        ),
        RuntimeMaintenanceCommand::Review,
    ] {
        fixture
            .processor
            .apply_command(&mut fixture.app, RuntimeCommand::Maintenance(command))
            .await
            .unwrap();
        assert_eq!(fixture.processor.session_id(), session);
        assert_eq!(
            fixture
                .app
                .bottom_pane
                .running_task
                .as_ref()
                .unwrap()
                .handle
                .id(),
            task_id
        );
        assert_eq!(
            fixture.app.bottom_pane.notice.as_deref(),
            Some("Wait for review preparation to finish or cancel it first.")
        );
    }
    fixture
        .processor
        .apply_command(
            &mut fixture.app,
            RuntimeCommand::SetPermissionMode(crate::tui::state::PermissionMode::ReadOnly),
        )
        .await
        .unwrap();
    assert!(fixture.app.pending_permission_mode.is_some());
    result
        .send(Ok(CapturedDiff {
            text: "+change".into(),
            truncated: false,
        }))
        .unwrap();
    fixture.finish().await;
    assert!(fixture.app.pending_permission_mode.is_none());
    assert_eq!(
        fixture.app.effective_permission_mode(),
        crate::tui::state::PermissionMode::ReadOnly
    );
    fixture.finish().await;
    assert_eq!(fixture.backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.processor.session_id(), session);
}
