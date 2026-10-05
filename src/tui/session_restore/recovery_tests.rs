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
    app.finish_resume_query_for_test().await;
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
    loading::finish_restore_for_test(&mut app, &mut slot)
        .await
        .unwrap_err();
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

#[tokio::test]
async fn startup_resume_failure_preserves_fresh_session() {
    for target in [
        StartupResumeTarget::Latest,
        StartupResumeTarget::ThreadId("target-thread".into()),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, mut slot) = fixture(&dir);
        let history = slot.as_ref().unwrap().history.clone();
        apply_startup_resume(&target, &mut app, &mut slot);
        loading::finish_restore_for_test(&mut app, &mut slot)
            .await
            .unwrap_err();
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

#[tokio::test]
async fn truncated_rollout_restores_thread_and_can_be_checkpointed_again() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, mut slot) = fixture(&dir);
    let sessions = slot.as_ref().unwrap().session_manager.clone();
    let path = sessions.storage_dir.join("target-thread/events.jsonl");
    let event = json!({"type": "plan_state", "explanation": "retained plan", "steps": []});
    std::fs::write(&path, format!("{event}\n{{\"type\":")).unwrap();
    restore_thread_by_id("target-thread", &mut app, &mut slot)
        .await
        .unwrap();
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
    restore_thread_by_id("target-thread", &mut app, &mut slot)
        .await
        .unwrap();
    assert_eq!(slot.as_ref().unwrap().history[0].content, json!("saved"));
}

#[tokio::test]
async fn large_restore_keeps_input_responsive_and_cancellation_keeps_the_agent() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use rara_state::state_db::PersistedTurnEntry;

    use crate::thread_store::ThreadRecorder;
    use crate::tui::testing::TuiHarness;

    let dir = tempfile::tempdir().unwrap();
    let (app, mut slot) = fixture(&dir);
    let sessions = slot.as_ref().unwrap().session_manager.clone();
    std::fs::write(sessions.storage_dir.join("target-thread/events.jsonl"), "").unwrap();
    let entries = (0..10_000)
        .map(|index| PersistedTurnEntry {
            role: "agent".into(),
            message: format!("Saved entry {index}"),
        })
        .collect::<Vec<_>>();
    ThreadRecorder::new(app.state_db.as_ref().unwrap())
        .persist_turn("target-thread", 0, &entries)
        .unwrap();
    let (release, wait) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let blocked = app
        .storage
        .as_ref()
        .unwrap()
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    ready.await.unwrap();
    let mut harness = TuiHarness::new(Default::default()).unwrap();
    *harness.app_mut() = app;
    request_restore_thread("target-thread", harness.app_mut(), &mut slot).unwrap();
    assert_eq!(slot.as_ref().unwrap().session_id, "current-thread");
    harness
        .press_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(harness.app().bottom_pane.input, "x");
    assert!(
        harness
            .screen_text(100, 30)
            .contains("Loading saved thread")
    );
    assert!(harness.app().is_busy());
    harness
        .press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(
        harness.app().bottom_pane.input,
        "x",
        "submission retains the draft while restore is pending"
    );
    assert_eq!(slot.as_ref().unwrap().session_id, "current-thread");

    harness
        .press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .unwrap();
    assert!(harness.app().pending_restore.is_none());
    assert_eq!(slot.as_ref().unwrap().session_id, "current-thread");
    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    harness.app_mut().flush_storage().await.unwrap();
    assert!(!poll_restore(harness.app_mut(), &mut slot));

    request_restore_thread("target-thread", harness.app_mut(), &mut slot).unwrap();
    loading::finish_restore_for_test(harness.app_mut(), &mut slot)
        .await
        .unwrap();
    assert_eq!(slot.as_ref().unwrap().session_id, "target-thread");
    assert_eq!(harness.app().committed_turns.len(), 1);
    assert_eq!(harness.app().committed_turns[0].entries.len(), 10_000);
    assert_eq!(
        harness.app().committed_turns[0].entries[9_999].message,
        "Saved entry 9999"
    );
    harness.app_mut().shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn permission_change_during_restore_applies_after_success_or_failure() {
    use crate::tui::state::PermissionMode;
    for corrupt in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let (mut app, mut slot) = fixture(&dir);
        if !corrupt {
            let sessions = &slot.as_ref().unwrap().session_manager;
            std::fs::write(sessions.storage_dir.join("target-thread/events.jsonl"), "").unwrap();
        }
        request_restore_thread("target-thread", &mut app, &mut slot).unwrap();
        crate::tui::runtime::request_permission_mode(
            &mut app,
            &mut slot,
            PermissionMode::FullAccess,
        );
        assert_eq!(
            app.pending_permission_mode,
            Some(PermissionMode::FullAccess)
        );
        let result = loading::finish_restore_for_test(&mut app, &mut slot).await;
        assert_eq!(result.is_err(), corrupt);
        assert!(app.pending_permission_mode.is_none());
        assert!(slot.as_ref().unwrap().full_access_mode);
        assert_eq!(
            slot.as_ref().unwrap().session_id,
            if corrupt {
                "current-thread"
            } else {
                "target-thread"
            }
        );
        app.shutdown_storage().await.unwrap();
    }
}

#[tokio::test]
async fn prepared_restore_replaces_session_local_interactions_without_clearing_live_data() {
    let dir = tempfile::tempdir().unwrap();
    let (mut app, mut slot) = fixture(&dir);
    let sessions = &slot.as_ref().unwrap().session_manager;
    std::fs::write(sessions.storage_dir.join("target-thread/events.jsonl"), "").unwrap();
    app.show_pending_plan_approval(Some("old-session-plan"));
    rara_persistence::thread_turn_log::append_rollout_fragment(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "target-thread",
        &rara_state::state_db::PersistedTurnEntry {
            role: "You".into(),
            message: "recover me".into(),
        },
    )
    .unwrap();
    restore_thread_by_id("target-thread", &mut app, &mut slot)
        .await
        .unwrap();
    assert!(!app.has_pending_plan_approval());
    assert!(app.snapshot.completed_interactions.is_empty());
    assert_eq!(app.active_turn.entries[0].message, "recover me");
    app.shutdown_storage().await.unwrap();
    let live = rara_persistence::thread_turn_log::load_live_entries(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "target-thread",
    );
    assert_eq!(
        live.iter()
            .map(|entry| entry.message.as_str())
            .collect::<Vec<_>>(),
        ["recover me", "Resumed thread target-thread."],
        "view replacement retains the recovery copy and appends one resume notice"
    );
}
