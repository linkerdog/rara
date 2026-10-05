use super::*;

fn empty_state() -> ThreadRuntimeState<'static> {
    ThreadRuntimeState {
        session_id: "named-thread",
        cwd: "/workspace",
        branch: "main",
        provider: "ollama",
        model: "qwen3",
        base_url: None,
        agent_mode: "execute",
        bash_approval: "suggestion",
        plan_explanation: None,
        prompt_runtime: PersistedPromptRuntimeState::default(),
        history_len: 0,
        transcript_len: 0,
        compact_state: PersistedCompactState::default(),
    }
}

#[test]
fn name_survives_restart_checkpoints_and_empty_thread_listing() -> Result<()> {
    let root = tempdir()?;
    let db = StateDb::new_for_root_dir(root.path().to_owned())?;
    let state = empty_state();
    let recorder = ThreadRecorder::new(&db);
    recorder.persist_runtime_state(&state)?;
    assert!(db.list_recent_thread_records(10)?.is_empty());
    recorder.rename_thread(state.session_id, "Investigate query plans")?;
    recorder.persist_runtime_state(&state)?;
    drop(db);

    let db = StateDb::new_for_root_dir(root.path().to_owned())?;
    let stored = thread_metadata::load_thread_record(&db.rollout_root(), state.session_id)?
        .expect("canonical metadata");
    assert_eq!(stored.title.as_deref(), Some("Investigate query plans"));
    assert_eq!(
        db.load_thread_record(state.session_id)?.unwrap().title,
        stored.title
    );
    let threads = ThreadStore::list_recent_threads_for_db(&db, 10)?;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0].metadata.title, stored.title);
    let sessions = SessionManager::new_for_rara_dir(root.path().to_owned())?;
    let markdown = ThreadStore::new(&sessions, &db).export_thread_markdown(state.session_id)?;
    assert!(markdown.contains("# Investigate query plans"));
    Ok(())
}

#[test]
fn legacy_schema_and_metadata_gain_an_optional_title() -> Result<()> {
    let root = tempdir()?;
    let db = StateDb::new_for_root_dir(root.path().to_owned())?;
    let state = empty_state();
    ThreadRecorder::new(&db).persist_runtime_state(&state)?;
    let path = db.path().to_owned();
    let metadata = db.rollout_root().join(state.session_id).join("thread.json");
    let legacy: Value = serde_json::from_slice(&fs::read(&metadata)?)?;
    assert!(legacy.get("title").is_none());
    drop(db);
    rusqlite::Connection::open(path)?.execute("ALTER TABLE sessions DROP COLUMN title", [])?;
    let db = StateDb::new_for_root_dir(root.path().to_owned())?;
    assert!(
        db.load_thread_record(state.session_id)?
            .unwrap()
            .title
            .is_none()
    );
    assert!(
        thread_metadata::load_thread_record(&db.rollout_root(), state.session_id)?
            .unwrap()
            .title
            .is_none()
    );
    ThreadRecorder::new(&db).rename_thread(state.session_id, "Migrated thread")?;
    assert_eq!(
        db.load_thread_record(state.session_id)?
            .unwrap()
            .title
            .as_deref(),
        Some("Migrated thread")
    );
    Ok(())
}

#[test]
fn invalid_names_leave_the_existing_record_unchanged() -> Result<()> {
    let root = tempdir()?;
    let db = StateDb::new_for_root_dir(root.path().to_owned())?;
    let state = empty_state();
    let recorder = ThreadRecorder::new(&db);
    recorder.persist_runtime_state(&state)?;
    recorder.rename_thread(state.session_id, "Original name")?;
    let path = db.rollout_root().join(state.session_id).join("thread.json");
    let before = fs::read(&path)?;
    for invalid in ["", "  ", "line\nbreak", "hidden\u{1b}", &"a".repeat(257)] {
        assert!(recorder.rename_thread(state.session_id, invalid).is_err());
        assert_eq!(fs::read(&path)?, before);
    }
    assert_eq!(
        db.load_thread_record(state.session_id)?
            .unwrap()
            .title
            .as_deref(),
        Some("Original name")
    );
    assert!(recorder.rename_thread("missing-thread", "Name").is_err());
    Ok(())
}
