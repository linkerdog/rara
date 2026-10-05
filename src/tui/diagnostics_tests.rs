use std::process::Command;
use std::sync::Arc;

use rara_persistence::thread_data::PersistedTurnEntry;
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;

use super::message_role::MessageRole;
use super::state::{NoticeLevel, RuntimeSnapshot};
use super::testing::TuiHarness;
use crate::diagnostics::TerminalDiagnostics;
use crate::thread_store::ThreadRecorder;

#[test]
fn library_recovery_does_not_write_over_tui_stderr() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tui::diagnostics_tests::recovery_diagnostics_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[tokio::test]
#[ignore = "isolated process captures library stderr"]
async fn recovery_diagnostics_child() {
    let diagnostics = TerminalDiagnostics::start().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let root = db.rollout_root();
    let entry = PersistedTurnEntry {
        role: "Agent".into(),
        message: "surviving transcript".into(),
    };
    thread_turn_log::append_rollout_fragment(&root, "thread", &entry).unwrap();
    let path = root.join("thread/live.jsonl");
    let mut contents = std::fs::read(&path).unwrap();
    contents.extend_from_slice(b"{invalid}\n");
    std::fs::write(&path, contents).unwrap();
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    harness.app_mut().diagnostics = Some(diagnostics.reader());
    harness.app_mut().snapshot.session_id = "thread".into();
    harness.app_mut().attach_state_db(db.clone());
    for recovered in thread_turn_log::load_live_entries(&root, "thread") {
        harness
            .app_mut()
            .push_entry(MessageRole::Agent, recovered.message);
    }
    rusqlite::Connection::open(db.path()).unwrap().execute_batch(
        "CREATE TRIGGER fail_turn_index BEFORE INSERT ON turns BEGIN SELECT RAISE(FAIL, 'injected index failure'); END;"
    ).unwrap();
    ThreadRecorder::new(&db)
        .persist_turn("thread", 0, &[entry])
        .unwrap();
    std::thread::spawn(|| log::warn!("background warning token=synthetic-secret-value"))
        .join()
        .unwrap();
    assert!(harness.app_mut().poll_diagnostics());
    assert_eq!(
        harness.app().notice().unwrap().level(),
        NoticeLevel::Warning
    );
    let messages = harness
        .app()
        .active_turn
        .entries
        .iter()
        .map(|e| e.message.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(messages.contains("Live transcript recovery incomplete"));
    assert!(messages.contains("canonical turn log advanced"));
    assert!(messages.contains("injected index failure"));
    assert!(messages.contains("background warning token=[REDACTED_SECRET]"));
    assert!(!messages.contains("synthetic-secret-value"));
    assert!(!harness.app_mut().poll_diagnostics());
    assert_eq!(
        thread_turn_log::load_turn_records(&root, "thread")
            .unwrap()
            .len(),
        1
    );
    assert!(
        harness
            .screen_text(100, 24)
            .contains("surviving transcript")
    );

    let summary = "Resumed thread. Already recorded recovery warning. Another recovery warning.";
    harness.app_mut().push_notice(NoticeLevel::Warning, summary);
    let entries = harness.app().active_turn.entries.len();
    log::warn!("Already recorded recovery warning.");
    assert!(!harness.app_mut().poll_diagnostics());
    assert_eq!(harness.app().notice_text(), Some(summary));
    assert_eq!(harness.app().active_turn.entries.len(), entries);

    // A diagnostic write failure gets one diagnostic of its own, not a new
    // record on every event-loop tick.
    harness.app_mut().flush_storage().await.unwrap();
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir(&path).unwrap();
    log::warn!("trigger a failing diagnostic write");
    assert!(harness.app_mut().poll_diagnostics());
    harness.app_mut().flush_storage().await.unwrap_err();
    assert!(harness.app_mut().poll_diagnostics());
    harness.app_mut().flush_storage().await.unwrap_err();
    assert!(!harness.app_mut().poll_diagnostics());
    std::fs::remove_dir(&path).unwrap();
    harness.app_mut().shutdown_storage().await.unwrap();
    drop(diagnostics);
}

#[test]
fn cli_logging_handoff_preserves_redaction_and_unread_messages() {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "tui::diagnostics_tests::diagnostic_handoff_child",
            "--ignored",
            "--nocapture",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("before terminal"));
    assert!(stderr.contains("unread after terminal"));
    assert!(stderr.contains("after terminal"));
    assert!(!stderr.contains("captured during terminal"));
    assert!(!stderr.contains("synthetic-secret-value"));
}

#[test]
#[ignore = "isolated process owns global logger"]
fn diagnostic_handoff_child() {
    crate::diagnostics::initialize_cli_logging().unwrap();
    log::warn!("before terminal token=synthetic-secret-value");
    {
        let diagnostics = TerminalDiagnostics::start().unwrap();
        assert!(TerminalDiagnostics::start().is_err());
        log::warn!("captured during terminal");
        assert_eq!(
            diagnostics.reader().drain()[0].message,
            "captured during terminal"
        );
        log::warn!("unread after terminal token=synthetic-secret-value");
    }
    log::warn!("after terminal token=synthetic-secret-value");
    let next = TerminalDiagnostics::start().unwrap();
    log::error!("next terminal");
    let records = next.reader().drain();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].level, log::Level::Error);
}
