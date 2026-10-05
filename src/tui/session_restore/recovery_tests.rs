use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState};
use rara_tools::tool::ToolManager;
use serde_json::json;

use super::*;
use crate::config::ConfigManager;
use crate::llm::{Message, MockLlm};
use crate::oauth::OAuthManager;
use crate::session::SessionManager;
use crate::tui::app_event::AppEvent;
use crate::tui::event_dispatch::dispatch_event_with_runtime;
use crate::tui::event_loop::StartupResumeTarget;
use crate::tui::state::{
    ListPickerKind, Overlay, PROVIDER_FAMILIES, ProviderFamily, RuntimeSnapshot,
};
use crate::tui::testing::FakeRuntimeClient;
use crate::workspace::WorkspaceMemory;

fn fixture(dir: &tempfile::TempDir) -> (TuiApp, Option<Agent>) {
    let root = dir.path().join("workspace");
    let data = dir.path().join("state");
    let sessions = Arc::new(SessionManager::new_for_rara_dir(data.clone()).unwrap());
    let db = Arc::new(StateDb::new_for_root_dir(data.clone()).unwrap());
    db.upsert_session(
        "target-thread",
        &root.display().to_string(),
        "main",
        "ollama",
        "test",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        0,
        &PersistedCompactState::default(),
    )
    .unwrap();
    sessions
        .save_session(
            "target-thread",
            &[Message {
                role: "user".into(),
                content: json!("saved"),
            }],
        )
        .unwrap();
    let events = sessions
        .storage_dir
        .join("target-thread")
        .join("events.jsonl");
    std::fs::write(events, "{invalid}\n").unwrap();
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &data.join("memory").display().to_string(),
        )),
        sessions.clone(),
        Arc::new(WorkspaceMemory::from_paths(root.clone(), data)),
    );
    agent.set_session_id("current-thread".into());
    agent.history.push(Message {
        role: "user".into(),
        content: json!("keep current history"),
    });
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    app.snapshot.session_id = "current-thread".into();
    app.snapshot.cwd = root.display().to_string();
    app.attach_state_db(db.clone());
    app.recent_threads = ThreadStore::new(&sessions, &db)
        .list_recent_threads(10)
        .unwrap();
    (app, Some(agent))
}

#[tokio::test]
async fn resume_failure_keeps_picker_and_current_session() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, mut slot) = fixture(&dir);
    let history = slot.as_ref().unwrap().history.clone();
    let oauth =
        Arc::new(OAuthManager::new_for_isolated_rara_home(dir.path().join("auth")).unwrap());
    let runtime = FakeRuntimeClient::new(RuntimeSnapshot::default());
    let overlay = Overlay::ListPicker(ListPickerKind::Resume);
    app.open_overlay(overlay);
    assert_eq!(
        crate::tui::list_picker::selected_resumable_thread_id(&app).as_deref(),
        Some("target-thread")
    );
    assert!(
        !dispatch_event_with_runtime(
            AppEvent::ApplyOverlaySelection,
            &mut app,
            &mut slot,
            &oauth,
            &runtime
        )
        .await
        .expect("resume errors must not exit the TUI")
    );
    assert_eq!(app.overlay, Some(overlay));
    assert_eq!(slot.as_ref().unwrap().session_id, "current-thread");
    assert_eq!(slot.as_ref().unwrap().history, history);
    assert_eq!(app.snapshot.session_id, "current-thread");
    assert!(app.notice_text().unwrap().contains("Could not resume"));
    assert!(runtime.commands().is_empty());
    assert!(
        !dispatch_event_with_runtime(
            AppEvent::CloseOverlay,
            &mut app,
            &mut slot,
            &oauth,
            &runtime
        )
        .await
        .unwrap()
    );
    assert!(app.overlay.is_none());
}

#[test]
fn startup_resume_failure_preserves_fresh_session() {
    for target in [
        StartupResumeTarget::Latest,
        StartupResumeTarget::ThreadId("target-thread".into()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, mut slot) = fixture(&dir);
        let history = slot.as_ref().unwrap().history.clone();
        apply_startup_resume(&target, &mut app, &mut slot);
        assert_eq!(slot.as_ref().unwrap().session_id, "current-thread");
        assert_eq!(slot.as_ref().unwrap().history, history);
        assert_eq!(app.snapshot.session_id, "current-thread");
        assert!(app.notice_text().unwrap().contains("Could not resume"));
        assert!(!app.is_busy());
    }
}

#[tokio::test]
async fn unreadable_credential_keeps_model_picker_open() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, mut slot) = fixture(&dir);
    let auth_home = dir.path().join("auth");
    let oauth = Arc::new(OAuthManager::new_for_isolated_rara_home(auth_home.clone()).unwrap());
    std::fs::create_dir(auth_home.join(".codex/auth.json")).unwrap();
    assert!(
        oauth.saved_auth_mode().is_err(),
        "credential read must fail"
    );
    let runtime = FakeRuntimeClient::new(RuntimeSnapshot::default());
    app.provider_picker_idx = PROVIDER_FAMILIES
        .iter()
        .position(|(provider, _, _)| *provider == ProviderFamily::Codex)
        .unwrap();
    let overlay = Overlay::ListPicker(ListPickerKind::Model);
    app.open_overlay(overlay);
    assert!(
        !dispatch_event_with_runtime(
            AppEvent::ApplyOverlaySelection,
            &mut app,
            &mut slot,
            &oauth,
            &runtime
        )
        .await
        .expect("credential errors must not exit the TUI")
    );
    assert_eq!(app.overlay, Some(overlay));
    assert!(slot.is_some());
    assert!(runtime.commands().is_empty());
    assert!(
        app.notice_text()
            .unwrap()
            .contains("Could not load saved credential")
    );
}

#[test]
fn truncated_rollout_restores_thread_and_can_be_checkpointed_again() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, mut slot) = fixture(&dir);
    let sessions = slot.as_ref().unwrap().session_manager.clone();
    let path = sessions.storage_dir.join("target-thread/events.jsonl");
    let event = json!({"type": "plan_state", "explanation": "retained plan", "steps": []});
    std::fs::write(&path, format!("{event}\n{{\"type\":")).unwrap();
    restore_thread_by_id("target-thread", &mut app, &mut slot).unwrap();
    assert_eq!(slot.as_ref().unwrap().session_id, "target-thread");
    assert_eq!(app.snapshot.session_id, "target-thread");
    assert_eq!(
        app.snapshot.plan_explanation.as_deref(),
        Some("retained plan")
    );
    let event: rara_persistence::thread_data::PersistedStructuredRolloutEvent =
        serde_json::from_value(event).unwrap();
    rara_persistence::thread_rollout_log::append_rollout_event_line(
        &sessions.storage_dir,
        "target-thread",
        &event,
    )
    .unwrap();
    restore_thread_by_id("target-thread", &mut app, &mut slot).unwrap();
    assert_eq!(slot.as_ref().unwrap().history[0].content, json!("saved"));
}
