use std::sync::{Arc, mpsc};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;

use crate::thread_io::ThreadIo;
use crate::tui::message_role::MessageRole;
use crate::tui::testing::TuiHarness;

#[tokio::test]
async fn blocked_store_keeps_production_key_handling_and_rendering_responsive() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let root = db.rollout_root();
    let io = ThreadIo::new(db.clone()).unwrap();
    let (release, wait) = mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let read = io
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), ready)
        .await
        .unwrap()
        .unwrap();

    let mut harness = TuiHarness::new(Default::default()).unwrap();
    harness.app_mut().snapshot.session_id = "test".into();
    harness.app_mut().state_db = Some(db);
    harness.app_mut().storage = Some(io);
    harness
        .app_mut()
        .push_entry(MessageRole::User, "visible while storage is blocked");
    tokio::time::timeout(
        Duration::from_secs(5),
        harness.press_key(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::NONE)),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(harness.app().bottom_pane.input, "x");
    assert!(
        harness
            .screen_text(100, 30)
            .contains("visible while storage is blocked")
    );
    assert!(
        !root.join("test").exists(),
        "the blocked store cannot have written yet"
    );
    release.send(()).unwrap();
    read.await.unwrap().unwrap();
    harness.app_mut().flush_storage().await.unwrap();
    assert_eq!(thread_turn_log::load_live_entries(&root, "test").len(), 1);
    harness
        .app_mut()
        .storage
        .as_mut()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

#[tokio::test]
async fn display_context_refresh_does_not_change_model_prompt_assembly() {
    use super::*;

    let dir = tempdir().unwrap();
    let root = dir.path().join("workspace");
    let data = dir.path().join("state");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&data).unwrap();
    std::fs::write(root.join("AGENTS.md"), "First workspace instructions.").unwrap();
    let agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &data.join("memory").display().to_string(),
        )),
        Arc::new(SessionManager::new_for_rara_dir(data.clone()).unwrap()),
        Arc::new(WorkspaceMemory::from_paths(root, data)),
    );
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    app.apply_runtime_snapshot(&agent, RuntimeExtensionSnapshot::default());
    assert_eq!(
        app.context_files_status(),
        Some("Loading workspace context...")
    );
    app.finish_context_files_for_test(&agent).await;
    assert_eq!(app.context_files_status(), None);

    let before = agent.shared_runtime_context();
    assert_eq!(
        app.snapshot.prompt_source_entries,
        before.prompt.source_entries
    );
    assert_eq!(
        app.snapshot.stable_instructions_budget,
        before.budget.stable_instructions_budget
    );
    assert_eq!(
        app.snapshot.memory_selection,
        before.retrieval.memory_selection
    );

    // Changing the owned runtime config invalidates display data immediately;
    // request assembly continues to use the actual config without waiting for UI reads.
    let mut agent = agent;
    let mut config = agent.prompt_config().clone();
    config.append_system_prompt = Some("Additional request instruction.".into());
    agent.set_prompt_config(config);
    app.apply_runtime_snapshot(&agent, RuntimeExtensionSnapshot::default());
    assert!(app.context_files_status().is_some());
    assert!(
        agent
            .assemble_turn_context()
            .prompt
            .effective_prompt
            .text
            .contains("Additional request instruction.")
    );
    app.finish_context_files_for_test(&agent).await;
    let after = agent.shared_runtime_context();
    assert_eq!(
        app.snapshot.prompt_source_entries,
        after.prompt.source_entries
    );
    assert_eq!(
        app.snapshot.stable_instructions_budget,
        after.budget.stable_instructions_budget
    );
}

#[tokio::test]
async fn resume_search_discards_blocked_previous_queries() {
    use super::*;
    let dir = tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    app.attach_state_db(db);
    app.snapshot.cwd = dir.path().display().to_string();
    for id in ["alpha-thread", "beta-thread"] {
        app.snapshot.session_id = id.into();
        app.snapshot.history_len = 1;
        app.persist_runtime_state();
    }
    app.flush_storage().await.unwrap();
    app.snapshot.session_id = "current-thread".into();
    let (release, wait) = mpsc::channel();
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
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    app.poll_resume_queries();
    app.insert_active_input_text("beta");
    assert!(crate::tui::list_picker::selected_resumable_thread_id(&app).is_none());
    assert!(app.resume_query.loading);
    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    app.finish_resume_query_for_test().await;
    assert_eq!(
        crate::tui::list_picker::selected_resumable_thread_id(&app).as_deref(),
        Some("beta-thread")
    );
    assert_eq!(app.recent_threads.len(), 1);
    app.shutdown_storage().await.unwrap();
}
