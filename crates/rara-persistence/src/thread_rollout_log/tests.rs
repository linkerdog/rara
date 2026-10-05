use super::*;

fn event() -> PersistedStructuredRolloutEvent {
    PersistedStructuredRolloutEvent::PlanState {
        recorded_at: None,
        explanation: Some("complete record".into()),
        steps: Vec::new(),
    }
}

#[test]
fn interrupted_tail_restores_prefix_and_preserves_original_before_append() -> Result<()> {
    for tail in [
        b"{\"type\":\"plan_state\",\"explanation\":\"cut".as_slice(),
        b"{\"type\":\"plan_state\",\"explanation\":\"\xe4\xb8",
    ] {
        let dir = tempfile::tempdir()?;
        append_rollout_event_line(dir.path(), "thread", &event())?;
        let path = rollout_events_log_path(dir.path(), "thread");
        let mut content = fs::read(&path)?;
        content.extend_from_slice(tail);
        fs::write(&path, &content)?;

        assert_eq!(load_rollout_events(dir.path(), "thread")?.len(), 1);
        assert_eq!(fs::read(&path)?, content, "read must not rewrite the log");
        append_rollout_event_line(dir.path(), "thread", &event())?;
        assert_eq!(load_rollout_events(dir.path(), "thread")?.len(), 2);
        let backups = fs::read_dir(path.parent().expect("parent"))?
            .collect::<std::io::Result<Vec<_>>>()?
            .into_iter()
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .starts_with("events.jsonl.recovery-")
            })
            .collect::<Vec<_>>();
        assert_eq!(backups.len(), 1);
        assert_eq!(fs::read(backups[0].path())?, content);
    }
    Ok(())
}

#[test]
fn complete_unterminated_record_gets_a_separator_before_append() -> Result<()> {
    let dir = tempfile::tempdir()?;
    append_rollout_event_line(dir.path(), "thread", &event())?;
    let path = rollout_events_log_path(dir.path(), "thread");
    fs::write(&path, serde_json::to_vec(&event())?)?;
    assert_eq!(load_rollout_events(dir.path(), "thread")?.len(), 1);
    append_rollout_event_line(dir.path(), "thread", &event())?;
    assert_eq!(load_rollout_events(dir.path(), "thread")?.len(), 2);
    Ok(())
}

#[test]
fn middle_corruption_and_invalid_complete_records_are_errors() -> Result<()> {
    let dir = tempfile::tempdir()?;
    append_rollout_event_line(dir.path(), "thread", &event())?;
    let path = rollout_events_log_path(dir.path(), "thread");
    let valid = serde_json::to_string(&event())?;
    for tail in [
        format!("{{\n{valid}\n"),
        "{\n".into(),
        "{invalid}".into(),
        "{\"type\":\"unknown\"}".into(),
    ] {
        let content = format!("{valid}\n{tail}");
        fs::write(&path, &content)?;
        let error = load_rollout_events(dir.path(), "thread").expect_err("corruption must fail");
        let message = format!("{error:#}");
        assert!(message.contains("events.jsonl"), "{message}");
        assert!(message.contains("line 2"), "{message}");
        assert_eq!(fs::read_to_string(&path)?, content);
    }
    Ok(())
}
