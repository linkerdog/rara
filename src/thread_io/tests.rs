use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc,
};
use std::time::Duration;

use anyhow::Result;
use rara_persistence::thread_data::PersistedTurnEntry;
use rara_persistence::thread_turn_log;
use rara_state::state_db::StateDb;

use super::worker::WriteStore;
use super::*;

#[derive(Default)]
struct RecordingStore {
    writes: Arc<Mutex<Vec<String>>>,
    fail_commit: Arc<AtomicBool>,
}

impl WriteStore for RecordingStore {
    fn write(&self, operation: &WriteOperation) -> Result<()> {
        let description = match operation {
            WriteOperation::Runtime(state) => format!("runtime:{}", state.history_len),
            WriteOperation::AppendLive { entries, .. } => format!("live:{}", entries.len()),
            WriteOperation::ReplaceLive { entries, .. } => format!("replace:{}", entries.len()),
            WriteOperation::CommitTurn { ordinal, .. } => {
                anyhow::ensure!(
                    !self.fail_commit.load(Ordering::SeqCst),
                    "scripted commit failure"
                );
                format!("commit:{ordinal}")
            }
            WriteOperation::ClearLive { .. } => "clear".into(),
        };
        self.writes.lock().unwrap().push(description);
        Ok(())
    }
}

fn entry(message: &str) -> PersistedTurnEntry {
    PersistedTurnEntry {
        role: "user".into(),
        message: message.into(),
    }
}

fn checkpoint(history_len: usize) -> WriteOperation {
    WriteOperation::Runtime(Box::new(RuntimeCheckpoint {
        session_id: "test".into(),
        cwd: "/workspace".into(),
        branch: "main".into(),
        provider: "mock".into(),
        model: "mock".into(),
        base_url: None,
        agent_mode: "execute".into(),
        bash_approval: "always".into(),
        plan_explanation: None,
        prompt_runtime: Default::default(),
        history_len,
        transcript_len: 0,
        compact_state: Default::default(),
        plan_steps: Vec::new(),
        interactions: Vec::new(),
        rollout: Vec::new(),
    }))
}

#[tokio::test]
async fn accepted_command_survives_a_dropped_receipt_and_shutdown_drains_it() {
    let store = RecordingStore::default();
    let writes = store.writes.clone();
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    let (release, wait) = mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let blocked = io
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    ready.await.unwrap();
    io.submit(checkpoint(1)).unwrap();
    let observed = writes.clone();
    let receipt = io
        .execute(move || {
            assert_eq!(*observed.lock().unwrap(), ["runtime:1"]);
            observed.lock().unwrap().push("command".into());
            Ok(())
        })
        .unwrap();
    drop(receipt);
    io.submit(checkpoint(2)).unwrap();
    release.send(()).unwrap();
    blocked.await.unwrap().unwrap();
    io.shutdown().await.unwrap();
    assert_eq!(
        *writes.lock().unwrap(),
        ["runtime:1", "command", "runtime:2"]
    );
}

#[tokio::test]
async fn failed_preceding_write_rejects_a_command_without_executing_it() {
    let store = RecordingStore::default();
    let fail = store.fail_commit.clone();
    fail.store(true, Ordering::SeqCst);
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    io.submit(WriteOperation::CommitTurn {
        session_id: "test".into(),
        ordinal: 0,
        entries: vec![entry("keep me")],
    })
    .unwrap();
    let executed = Arc::new(AtomicBool::new(false));
    let observed = executed.clone();
    let result = io
        .execute(move || {
            observed.store(true, Ordering::SeqCst);
            Ok(())
        })
        .unwrap()
        .await
        .unwrap();
    assert!(
        result
            .unwrap_err()
            .to_string()
            .contains("scripted commit failure")
    );
    assert!(!executed.load(Ordering::SeqCst));
    fail.store(false, Ordering::SeqCst);
    io.shutdown().await.unwrap();
}

#[tokio::test]
async fn adjacent_writes_batch_without_crossing_read_or_commit_barriers() {
    let store = RecordingStore::default();
    let writes = store.writes.clone();
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    let (release, wait) = mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let first = io
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    ready.await.unwrap();
    io.submit(checkpoint(1)).unwrap();
    io.submit(checkpoint(2)).unwrap();
    for message in ["one", "two"] {
        io.submit(WriteOperation::AppendLive {
            session_id: "test".into(),
            entries: vec![entry(message)],
        })
        .unwrap();
    }
    io.submit(WriteOperation::CommitTurn {
        session_id: "test".into(),
        ordinal: 0,
        entries: vec![entry("one"), entry("two")],
    })
    .unwrap();
    io.submit(checkpoint(3)).unwrap();
    let boundary = io.read(|| Ok(())).unwrap();
    io.submit(checkpoint(4)).unwrap();
    release.send(()).unwrap();
    first.await.unwrap().unwrap();
    boundary.await.unwrap().unwrap();
    io.shutdown().await.unwrap();
    assert_eq!(
        *writes.lock().unwrap(),
        ["runtime:2", "live:2", "commit:0", "runtime:3", "runtime:4"]
    );
}

#[tokio::test]
async fn failed_commit_retains_work_and_prevents_later_clear_until_retry_succeeds() {
    let store = RecordingStore::default();
    let writes = store.writes.clone();
    let fail = store.fail_commit.clone();
    fail.store(true, Ordering::SeqCst);
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    io.submit(WriteOperation::CommitTurn {
        session_id: "test".into(),
        ordinal: 0,
        entries: vec![entry("keep me")],
    })
    .unwrap();
    io.submit(WriteOperation::ClearLive {
        session_id: "test".into(),
    })
    .unwrap();
    assert!(io.flush().await.is_err());
    assert!(writes.lock().unwrap().is_empty());
    assert!(
        io.status()
            .error
            .as_deref()
            .unwrap()
            .contains("scripted commit failure")
    );
    fail.store(false, Ordering::SeqCst);
    io.shutdown().await.unwrap();
    assert_eq!(*writes.lock().unwrap(), ["commit:0", "clear"]);
    assert!(io.status().error.is_none());
}

#[tokio::test]
async fn database_failure_keeps_live_recovery_copy_and_retry_commits_one_ordinal() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(StateDb::new_for_root_dir(dir.path().join("state")).unwrap());
    let root = db.rollout_root();
    let mut io = ThreadIo::new(db).unwrap();
    io.submit(checkpoint(0)).unwrap();
    let entries = vec![entry("keep me"), entry("until committed")];
    io.submit(WriteOperation::AppendLive {
        session_id: "test".into(),
        entries: entries.clone(),
    })
    .unwrap();
    io.flush().await.unwrap();
    let bad_path = thread_turn_log::turn_log_path(&root, "test");
    std::fs::create_dir(&bad_path).unwrap();
    io.submit(WriteOperation::CommitTurn {
        session_id: "test".into(),
        ordinal: 0,
        entries,
    })
    .unwrap();
    assert!(io.flush().await.is_err());
    assert_eq!(thread_turn_log::load_live_entries(&root, "test").len(), 2);
    std::fs::remove_dir(&bad_path).unwrap();
    io.shutdown().await.unwrap();
    let turns = thread_turn_log::load_turn_records(&root, "test").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].entries.len(), 2);
    assert!(thread_turn_log::load_live_entries(&root, "test").is_empty());
}

#[tokio::test]
async fn a_panicking_read_does_not_drop_later_accepted_writes() {
    let store = RecordingStore::default();
    let writes = store.writes.clone();
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    let (release, wait) = mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let failed = io
        .read(move || -> Result<()> {
            entered.send(()).unwrap();
            wait.recv()?;
            panic!("scripted read failure");
        })
        .unwrap();
    ready.await.unwrap();
    io.submit(checkpoint(7)).unwrap();
    release.send(()).unwrap();
    assert!(
        failed
            .await
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("read panicked")
    );
    io.shutdown().await.unwrap();
    assert_eq!(*writes.lock().unwrap(), ["runtime:7"]);
}

#[tokio::test]
async fn a_panicking_write_keeps_the_operation_before_later_clears() {
    struct PanickingStore(AtomicBool, RecordingStore);
    impl WriteStore for PanickingStore {
        fn write(&self, operation: &WriteOperation) -> Result<()> {
            assert!(
                !self.0.swap(false, Ordering::SeqCst),
                "scripted write panic"
            );
            self.1.write(operation)
        }
    }
    let store = PanickingStore(AtomicBool::new(true), RecordingStore::default());
    let writes = store.1.writes.clone();
    let mut io = ThreadIo::with_store_on_barriers(Box::new(store)).unwrap();
    io.submit(checkpoint(8)).unwrap();
    io.submit(WriteOperation::ClearLive {
        session_id: "test".into(),
    })
    .unwrap();
    assert!(
        io.flush()
            .await
            .unwrap_err()
            .to_string()
            .contains("panicked")
    );
    assert!(writes.lock().unwrap().is_empty());
    io.shutdown().await.unwrap();
    assert_eq!(*writes.lock().unwrap(), ["runtime:8", "clear"]);
}
