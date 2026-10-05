use super::*;
use crate::config::ConfigManager;

fn app() -> (tempfile::TempDir, TuiApp) {
    let dir = tempfile::tempdir().unwrap();
    let app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    (dir, app)
}

#[test]
fn expiry_retains_redacted_history_and_requests_only_one_change() {
    let (_dir, mut app) = app();
    let now = Instant::now();
    app.publish_notice(
        NoticeLevel::Error,
        "token=synthetic-secret-value".into(),
        NoticeOwner::General,
        NoticeRecord::Plain,
        now,
    );
    assert_eq!(app.notice().unwrap().level(), NoticeLevel::Error);
    assert_eq!(app.notice_text(), Some("token=[REDACTED_SECRET]"));
    let message = app.active_turn.entries.last().unwrap().message.clone();
    assert_eq!(Some(message.as_str()), app.notice_text());
    assert!(!app.expire_notice(now + NOTICE_LIFETIME - Duration::from_nanos(1)));
    assert!(app.expire_notice(now + NOTICE_LIFETIME));
    assert!(app.notice().is_none());
    assert!(!app.expire_notice(now + NOTICE_LIFETIME));
    assert_eq!(app.active_turn.entries.last().unwrap().message, message);
}

#[test]
fn replacing_a_notice_uses_its_own_deadline() {
    let (_dir, mut app) = app();
    let now = Instant::now();
    app.publish_notice(
        NoticeLevel::Info,
        "first".into(),
        NoticeOwner::General,
        NoticeRecord::Plain,
        now,
    );
    app.publish_notice(
        NoticeLevel::Warning,
        "second".into(),
        NoticeOwner::General,
        NoticeRecord::Plain,
        now + Duration::from_secs(4),
    );
    assert!(!app.expire_notice(now + NOTICE_LIFETIME));
    assert_eq!(app.notice_text(), Some("second"));
    assert!(app.expire_notice(now + Duration::from_secs(4) + NOTICE_LIFETIME));
    let messages = app
        .active_turn
        .entries
        .iter()
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(messages, ["first", "second"]);
}

#[test]
fn clearing_a_draft_uses_notice_ownership_instead_of_message_equality() {
    let (_dir, mut app) = app();
    app.bottom_pane.handle_paste_burst_chunk("draft");
    assert!(app.flush_composer_paste());
    let paste = app.notice_text().unwrap().to_string();
    app.push_notice(NoticeLevel::Warning, paste.clone());
    app.clear_composer();
    assert_eq!(app.notice_text(), Some(paste.as_str()));
    assert_eq!(app.notice().unwrap().level(), NoticeLevel::Warning);
    app.bottom_pane
        .handle_paste_burst_chunk("replacement draft");
    app.flush_composer_paste();
    app.clear_composer();
    assert!(app.notice().is_none());
    assert_eq!(app.active_turn.entries.len(), 3);
}

#[test]
fn classified_notices_record_their_existing_presentation_kind_once() {
    let (_dir, mut app) = app();
    app.push_system_notice(
        NoticeLevel::Error,
        "OAuth failed: token=synthetic-secret-value",
        SystemMessageKind::OAuth,
    );
    assert_eq!(app.active_turn.entries.len(), 1);
    let entry = &app.active_turn.entries[0];
    assert_eq!(Some(entry.message.as_str()), app.notice_text());
    assert!(matches!(
        entry.payload,
        Some(crate::tui::state::TranscriptEntryPayload::System(
            SystemMessageKind::OAuth
        ))
    ));
}

#[test]
fn expired_notices_remain_redacted_in_live_and_committed_storage() {
    use rara_persistence::thread_turn_log;
    use rara_state::state_db::StateDb;

    let (dir, mut app) = app();
    let state_db =
        std::sync::Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let root = state_db.rollout_root();
    app.attach_state_db(state_db);
    app.snapshot.session_id = "notice-history".into();
    app.push_notice(NoticeLevel::Error, "token=synthetic-secret-value");
    assert!(app.expire_notice(Instant::now() + NOTICE_LIFETIME));
    let live = thread_turn_log::load_live_entries(&root, "notice-history");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].role, "System");
    assert_eq!(live[0].message, "token=[REDACTED_SECRET]");

    app.finalize_active_turn();
    let turns = thread_turn_log::load_turn_records(&root, "notice-history").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].entries.len(), 1);
    assert_eq!(turns[0].entries[0].role, live[0].role);
    assert_eq!(turns[0].entries[0].message, live[0].message);
    assert!(thread_turn_log::load_live_entries(&root, "notice-history").is_empty());
}
