use std::sync::Arc;

use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState};

use super::*;
use crate::config::ConfigManager;
use crate::tui::state::{ListPickerKind, Overlay};

fn session(db: &StateDb, id: &str, cwd: &str) {
    db.upsert_session(
        id,
        cwd,
        "main",
        "test",
        "model",
        None,
        "execute",
        "suggestion",
        None,
        &PersistedPromptRuntimeState::default(),
        1,
        1,
        &PersistedCompactState::default(),
    )
    .unwrap();
}

fn fixture() -> (tempfile::TempDir, Arc<StateDb>, TuiApp) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    app.attach_state_db(db.clone());
    app.snapshot.session_id = "current".into();
    app.snapshot.cwd = "/a/app".into();
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    (dir, db, app)
}

fn ids(app: &TuiApp) -> Vec<&str> {
    app.recent_threads
        .iter()
        .map(|thread| thread.metadata.session_id.as_str())
        .collect()
}

#[tokio::test]
async fn automatic_scope_falls_back_only_when_no_other_cwd_session_exists() {
    let (_dir, db, mut app) = fixture();
    session(&db, "current", "/a/app");
    session(&db, "foreign", "/b/app");
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["foreign"]);
    assert_eq!(app.resume_query.scope_label(), "all (auto)");
    app.refresh_recent_threads_for_resume_picker();
    app.toggle_resume_scope();
    app.finish_resume_query_for_test().await;
    assert!(ids(&app).is_empty(), "explicit cwd must not fall back");
    app.toggle_resume_scope();
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["foreign"]);

    session(&db, "local", "/a/app");
    app.resume_query.scope = ResumeScope::Automatic;
    app.insert_active_input_text("foreign");
    app.finish_resume_query_for_test().await;
    assert!(ids(&app).is_empty(), "a search miss must not change scope");
    assert_eq!(app.resume_query.scope_label(), "cwd (auto)");
    app.toggle_resume_scope();
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["foreign"]);
    app.shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn lazy_pages_preserve_selection_and_search_reaches_beyond_recent_limit() {
    let (_dir, db, mut app) = fixture();
    for i in 0..213 {
        session(&db, &format!("saved-{i:03}"), "/a/app");
    }
    app.finish_resume_query_for_test().await;
    assert_eq!(app.recent_threads.len(), 50);
    while app.resume_query.has_more() {
        let loaded = app.recent_threads.len();
        app.resume_picker_idx = loaded - 1;
        app.move_resume_selection(1);
        assert_eq!(app.recent_threads.len(), loaded);
        assert!(crate::tui::list_picker::selected_resumable_thread_id(&app).is_some());
        app.finish_resume_query_for_test().await;
        assert_eq!(app.resume_picker_idx, loaded);
    }
    assert_eq!(ids(&app).len(), 213);
    assert_eq!(ids(&app).into_iter().collect::<HashSet<_>>().len(), 213);
    app.insert_active_input_text("saved-000");
    assert!(app.recent_threads.is_empty());
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["saved-000"]);
    app.shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn pending_page_cannot_cross_search_or_workspace_boundaries() {
    let (_dir, db, mut app) = fixture();
    for i in 0..51 {
        session(&db, &format!("saved-{i:03}"), "/a/app");
    }
    session(&db, "foreign", "/b/app");
    app.finish_resume_query_for_test().await;
    app.resume_picker_idx = 49;
    app.move_resume_selection(1);
    app.poll_resume_queries();
    let old = app.resume_query.pending.take().unwrap();
    app.insert_active_input_text("saved-050");
    app.finish_resume_query(old.request, old.receiver.await.unwrap());
    assert!(app.recent_threads.is_empty());
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["saved-050"]);

    app.clear_resume_search();
    app.poll_resume_queries();
    let old = app.resume_query.pending.take().unwrap();
    app.snapshot.cwd = "/b/app".into();
    app.finish_resume_query(old.request, old.receiver.await.unwrap());
    assert!(app.recent_threads.is_empty());
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["foreign"]);
    app.shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn query_failure_is_visible_and_refresh_recovers() {
    let (_dir, db, mut app) = fixture();
    session(&db, "local", "/a/app");
    app.poll_resume_queries();
    let pending = app.resume_query.pending.take().unwrap();
    app.finish_resume_query(
        pending.request,
        Err(anyhow::anyhow!("scripted query failure")),
    );
    assert!(
        app.resume_query
            .error
            .as_deref()
            .unwrap()
            .contains("scripted query failure")
    );
    assert!(!app.resume_query.loading);
    assert!(
        app.bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("scripted query failure")
    );
    app.refresh_recent_threads_for_resume_picker();
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["local"]);
    assert!(app.resume_query.error.is_none());
    app.shutdown_storage().await.unwrap();
}

#[tokio::test]
async fn loaded_page_cursor_cannot_cross_a_runtime_source_change() {
    let (_dir, db, mut app) = fixture();
    for i in 0..51 {
        session(&db, &format!("saved-{i:03}"), "/a/app");
    }
    session(&db, "foreign", "/b/app");
    session(&db, "next-current", "/b/app");
    app.finish_resume_query_for_test().await;
    assert_eq!(app.recent_threads.len(), 50);
    app.resume_picker_idx = 49;
    app.snapshot.cwd = "/b/app".into();
    app.snapshot.session_id = "next-current".into();
    app.move_resume_selection(1);
    app.finish_resume_query_for_test().await;
    assert_eq!(ids(&app), ["foreign"]);
    assert!(!app.resume_query.has_more());
    app.shutdown_storage().await.unwrap();
}
