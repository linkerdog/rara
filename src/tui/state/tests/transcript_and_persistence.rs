use super::*;
use crate::tui::message_role::MessageRole;

#[test]
fn resume_picker_refreshes_recent_threads_on_open() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    app.attach_state_db(std::sync::Arc::new(state_db));

    assert!(app.recent_threads.is_empty());

    app.state_db
        .as_ref()
        .expect("state db")
        .upsert_session(
            "thread-1",
            "/tmp/workspace",
            "main",
            "ollama",
            "qwen3",
            None,
            "execute",
            "always",
            None,
            &PersistedPromptRuntimeState::default(),
            1,
            0,
            &PersistedCompactState::default(),
        )
        .expect("upsert thread");

    // Disable Cwd filter for deterministic test — the test sessions use
    // different workspace directories than the test process cwd.
    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));

    assert_eq!(app.recent_threads.len(), 1);
    assert_eq!(app.recent_threads[0].metadata.session_id, "thread-1");
    assert_eq!(app.resume_picker_idx, 0);
}

#[test]
fn resume_picker_loads_more_than_legacy_twenty_thread_cap() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    app.attach_state_db(std::sync::Arc::new(state_db));

    for idx in 0..25 {
        app.state_db
            .as_ref()
            .expect("state db")
            .upsert_session(
                format!("thread-{idx:02}").as_str(),
                "/tmp/workspace",
                "main",
                "ollama",
                "qwen3",
                None,
                "execute",
                "always",
                None,
                &PersistedPromptRuntimeState::default(),
                1,
                0,
                &PersistedCompactState::default(),
            )
            .expect("upsert thread");
    }

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));

    assert_eq!(app.recent_threads.len(), 25);
}

#[test]
fn resume_picker_search_filters_and_clear_restores_threads() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    app.attach_state_db(std::sync::Arc::new(state_db));

    for (thread_id, branch, model) in [
        ("thread-alpha", "feature/resume-search", "qwen3"),
        ("thread-beta", "main", "gpt-5.2"),
    ] {
        app.state_db
            .as_ref()
            .expect("state db")
            .upsert_session(
                thread_id,
                "/tmp/workspace",
                branch,
                "codex",
                model,
                None,
                "execute",
                "always",
                None,
                &PersistedPromptRuntimeState::default(),
                1,
                0,
                &PersistedCompactState::default(),
            )
            .expect("upsert thread");
    }

    app.open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    assert_eq!(app.recent_threads.len(), 2);

    for c in "resume-search".chars() {
        app.push_resume_search_char(c);
    }

    assert_eq!(app.resume_search_query, "resume-search");
    assert_eq!(app.resume_picker_idx, 0);
    assert_eq!(app.recent_threads.len(), 1);
    assert_eq!(app.recent_threads[0].metadata.session_id, "thread-alpha");

    app.clear_resume_search();

    assert!(app.resume_search_query.is_empty());
    assert_eq!(app.resume_picker_idx, 0);
    assert_eq!(app.recent_threads.len(), 2);
}

#[test]
fn finalize_agent_stream_updates_latest_committed_turn_when_final_text_arrives_late() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.committed_turns.push(TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "\u{4f60}\u{597d}".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "\u{4f60}\u{597d}\u{ff01}".into(),
                payload: None,
            },
        ],
    });

    app.finalize_agent_stream(Some("\u{4f60}\u{597d}\u{ff01}\u{6709}\u{4ec0}\u{4e48}\u{6211}\u{53ef}\u{4ee5}\u{5e2e}\u{4f60}\u{7684}\u{ff1f}".into()));

    assert!(app.active_turn.entries.is_empty());
    assert_eq!(
        app.committed_turns
            .last()
            .and_then(|turn| turn.entries.last())
            .map(|entry| entry.message.as_str()),
        Some(
            "\u{4f60}\u{597d}\u{ff01}\u{6709}\u{4ec0}\u{4e48}\u{6211}\u{53ef}\u{4ee5}\u{5e2e}\u{4f60}\u{7684}\u{ff1f}"
        )
    );
    assert_eq!(
        app.committed_turns.last().map(|turn| turn
            .entries
            .iter()
            .filter(|entry| entry.role == MessageRole::Agent)
            .count()),
        Some(1)
    );
}

#[test]
fn streamed_agent_output_scrubs_internal_runtime_blocks_before_commit() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_delta("Visible answer.\n");
    app.append_agent_delta("<agent_runtime>\n{\"phase\":\"tool_results_available\"}");
    let live_text = app
        .agent_stream_lines()
        .expect("agent stream")
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert!(live_text.contains("Visible answer."));
    assert!(!live_text.contains("agent_runtime"));
    assert!(!live_text.contains("tool_results_available"));

    app.append_agent_delta("\n</agent_runtime>\nFinal answer.");
    app.finalize_agent_stream(None);

    let message = app
        .active_turn
        .entries
        .iter()
        .find(|entry| entry.role == MessageRole::Agent)
        .map(|entry| entry.message.as_str())
        .expect("agent message");
    assert!(message.contains("Visible answer."));
    assert!(message.contains("Final answer."));
    assert!(!message.contains("agent_runtime"));
    assert!(!message.contains("tool_results_available"));
}

#[test]
fn streamed_agent_output_appends_visible_text_after_internal_block() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_delta("Visible before.\n");
    app.append_agent_delta("<agent_runtime>\nhidden");
    app.append_agent_delta("\n</agent_runtime>\nVisible after.");

    let live_text = app
        .agent_stream_lines()
        .expect("agent stream")
        .iter()
        .flat_map(|line| line.spans.iter())
        .map(|span| span.content.as_ref())
        .collect::<String>();
    assert_eq!(live_text.matches("Visible before.").count(), 1);
    assert_eq!(live_text.matches("Visible after.").count(), 1);
    assert!(!live_text.contains("agent_runtime"));
    assert!(!live_text.contains("hidden"));
}

#[test]
fn finalized_agent_stream_does_not_replace_agent_text_before_tool_boundary() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.push_entry(MessageRole::User, "Fix the rendering order");

    app.append_agent_delta("First assistant segment.");
    app.finalize_agent_stream(None);
    app.push_entry(MessageRole::Running, "Run cargo check");
    app.append_agent_delta("Second assistant segment.");
    app.finalize_agent_stream(None);

    let agent_entries = app
        .active_turn
        .entries
        .iter()
        .filter(|entry| entry.role == MessageRole::Agent)
        .map(|entry| entry.message.as_str())
        .collect::<Vec<_>>();
    assert_eq!(
        agent_entries,
        vec!["First assistant segment.", "Second assistant segment."]
    );

    let first_agent = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "First assistant segment.")
        .unwrap();
    let running = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "Run cargo check")
        .unwrap();
    let second_agent = app
        .active_turn
        .entries
        .iter()
        .position(|entry| entry.message == "Second assistant segment.")
        .unwrap();
    assert!(first_agent < running);
    assert!(running < second_agent);
}

#[test]
fn committed_turn_keeps_thinking_before_final_agent_message() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.push_entry(MessageRole::User, "Explain the ordering bug");

    app.append_agent_thinking_delta("Trace the event stream.");
    app.append_agent_delta("The transcript has two ordering sources.");
    app.finalize_active_turn();

    let entries = &app.committed_turns[0].entries;
    let roles = entries
        .iter()
        .map(|entry| entry.role.as_str())
        .collect::<Vec<_>>();
    assert_eq!(roles, vec!["You", "Thinking", "Agent"]);
    assert_eq!(entries[1].message, "Trace the event stream.");
    assert_eq!(
        entries[2].message,
        "The transcript has two ordering sources."
    );
}

#[test]
fn committed_turn_preserves_interleaved_assistant_segments() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.push_entry(MessageRole::User, "Inspect, run a tool, and finish");

    app.append_agent_thinking_delta("Inspect the implementation.");
    app.append_agent_delta("I found the relevant state transition.");
    app.finalize_agent_stream(None);
    app.push_tool_entry(
        Some("call-1"),
        "cargo_check",
        super::ToolTranscriptStatus::Completed,
        "cargo check passed",
    );
    app.append_agent_thinking_delta("Interpret the tool result.");
    app.append_agent_delta("The fix is ready.");
    app.finalize_active_turn();

    let entries = &app.committed_turns[0].entries;
    let ordered = entries
        .iter()
        .map(|entry| (entry.role.as_str(), entry.message.as_str()))
        .collect::<Vec<_>>();
    assert_eq!(
        ordered,
        vec![
            ("You", "Inspect, run a tool, and finish"),
            ("Thinking", "Inspect the implementation."),
            ("Agent", "I found the relevant state transition."),
            ("Tool Result", "cargo check passed"),
            ("Thinking", "Interpret the tool result."),
            ("Agent", "The fix is ready."),
        ]
    );
}

#[test]
fn flushed_agent_thinking_stream_scrubs_internal_runtime_blocks() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.append_agent_thinking_delta("Visible thought.\n");
    app.append_agent_thinking_delta("<agent_runtime>\n{\"phase\":\"tool_results_available\"}");
    app.append_agent_thinking_delta("\n</agent_runtime>\nNext thought.");
    app.finalize_agent_thinking_stream();

    assert_eq!(app.active_turn.entries.len(), 1);
    let entry = app.active_turn.entries.first().expect("thinking entry");
    assert_eq!(entry.role, MessageRole::Thinking);
    assert_eq!(entry.message, "Visible thought.\n\nNext thought.");
    assert!(!entry.message.contains("agent_runtime"));
    assert!(!entry.message.contains("tool_results_available"));
}

#[test]
fn live_progress_events_sanitize_terminal_controls() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    app.record_running_action("Run\r\u{1b}[31mcargo check\u{1b}[0m\u{8}");
    app.record_exploration_note("Read\tfile\u{7}");

    assert_eq!(app.active_turn.entries.len(), 2);
    assert_eq!(app.active_turn.entries[0].message, "Run\ncargo check");
    assert_eq!(app.active_turn.entries[1].message, "Read    file");
    assert!(!app.active_turn.entries[0].message.contains('\r'));
    assert!(!app.active_turn.entries[0].message.contains('\u{1b}'));
    assert!(!app.active_turn.entries[1].message.contains('\u{7}'));
}

#[test]
fn finalize_agent_stream_replaces_earlier_agent_entries_in_active_turn() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "\u{4f60}\u{597d}".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "\u{4f60}\u{597d}".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::System,
                message: "temporary runtime detail".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "\u{4f60}\u{597d}\u{ff01}".into(),
                payload: None,
            },
        ],
    }
    .into();

    app.finalize_agent_stream(Some("\u{4f60}\u{597d}\u{ff01}\u{6709}\u{4ec0}\u{4e48}\u{6211}\u{53ef}\u{4ee5}\u{5e2e}\u{4f60}\u{7684}\u{ff1f}".into()));

    let agent_entries = app
        .active_turn
        .entries
        .iter()
        .filter(|entry| entry.role == MessageRole::Agent)
        .collect::<Vec<_>>();
    assert_eq!(agent_entries.len(), 1);
    assert_eq!(
        agent_entries[0].message,
        "\u{4f60}\u{597d}\u{ff01}\u{6709}\u{4ec0}\u{4e48}\u{6211}\u{53ef}\u{4ee5}\u{5e2e}\u{4f60}\u{7684}\u{ff1f}"
    );
}

#[test]
fn restore_committed_turns_sets_inserted_counter_to_match() {
    let dir = tempdir().unwrap();
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");

    // Simulate session resume: restore N turns that were already on screen.
    let turns = vec![
        TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(MessageRole::User, "hello")],
        },
        TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(MessageRole::Agent, "hi there")],
        },
        TranscriptTurn {
            thinking_duration: None,
            entries: vec![TranscriptEntry::new(MessageRole::User, "bye")],
        },
    ];
    let n = turns.len();
    app.restore_committed_turns(turns);

    assert_eq!(app.committed_turns.len(), n);
    assert_eq!(app.active_turn.entries.len(), 0);
}

#[test]
fn active_turn_entries_write_and_clear_live_log() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.attach_state_db(std::sync::Arc::new(state_db));
    app.snapshot.session_id = "live-entry-session".to_string();

    app.push_entry(MessageRole::User, "hello");
    app.push_entry(MessageRole::Agent, "hi");

    let live_entries = thread_turn_log::load_live_entries(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "live-entry-session",
    );
    assert_eq!(live_entries.len(), 2);
    assert_eq!(live_entries[0].role, "You");
    assert_eq!(live_entries[0].message, "hello");
    assert_eq!(live_entries[1].role, "Agent");
    assert_eq!(live_entries[1].message, "hi");

    app.finalize_active_turn();

    let live_entries = thread_turn_log::load_live_entries(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "live-entry-session",
    );
    assert!(live_entries.is_empty());
    let turn_records = thread_turn_log::load_turn_records(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "live-entry-session",
    )
    .expect("turn records");
    assert_eq!(turn_records.len(), 1);
    assert_eq!(turn_records[0].entries.len(), 2);
}

#[test]
fn pending_plan_approval_persists_plan_ready_lifecycle() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "plan-ready-session".to_string();
    app.attach_state_db(std::sync::Arc::new(state_db));

    app.show_pending_plan_approval(None);

    let events = app
        .state_db
        .as_ref()
        .expect("state db")
        .load_rollout_events("plan-ready-session")
        .expect("rollout events");
    let ready_lifecycle = events.iter().find_map(|event| match event {
        PersistedStructuredRolloutEvent::RuntimeState { plan_lifecycle, .. } => plan_lifecycle
            .iter()
            .find(|lifecycle| lifecycle.phase == "plan_ready"),
        _ => None,
    });
    let ready_lifecycle = ready_lifecycle.expect("plan ready lifecycle");
    assert_eq!(
        ready_lifecycle.plan_path.as_deref(),
        Some(".rara/sessions/plan-ready-session/plan.md")
    );
    assert!(ready_lifecycle.submitted_at.is_some());
    assert_eq!(ready_lifecycle.decided_at, None);
}

#[test]
fn completed_plan_approval_persists_decision_lifecycle() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.snapshot.session_id = "plan-approved-session".to_string();
    app.attach_state_db(std::sync::Arc::new(state_db));

    app.record_completed_interaction_with_metadata(
        InteractionKind::PlanApproval,
        "Plan Decision",
        "copy can change",
        Some("plan_approval:approve".to_string()),
        Some("approved with tests".to_string()),
        Some("sha256:abc".to_string()),
    );

    let events = app
        .state_db
        .as_ref()
        .expect("state db")
        .load_rollout_events("plan-approved-session")
        .expect("rollout events");
    let approved_lifecycle = events.iter().find_map(|event| match event {
        PersistedStructuredRolloutEvent::RuntimeState { plan_lifecycle, .. } => plan_lifecycle
            .iter()
            .find(|lifecycle| lifecycle.phase == "plan_approved"),
        _ => None,
    });
    let approved_lifecycle = approved_lifecycle.expect("plan approved lifecycle");
    assert_eq!(approved_lifecycle.decision.as_deref(), Some("approve"));
    assert_eq!(
        approved_lifecycle.feedback.as_deref(),
        Some("approved with tests")
    );
    assert_eq!(approved_lifecycle.plan_hash.as_deref(), Some("sha256:abc"));
    assert_eq!(approved_lifecycle.submitted_at, None);
    assert!(approved_lifecycle.decided_at.is_some());
}

#[test]
fn push_system_redacts_live_log_entries() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.attach_state_db(std::sync::Arc::new(state_db));
    app.snapshot.session_id = "live-redaction-session".to_string();

    app.push_system(
        "token=supersecretvalue Authorization: Bearer abcdefghijklmnopqrstuvwxyz",
        SystemMessageKind::Other,
    );

    let live_entries = thread_turn_log::load_live_entries(
        &app.state_db.as_ref().unwrap().rollout_root(),
        "live-redaction-session",
    );
    assert_eq!(live_entries.len(), 1);
    assert!(live_entries[0].message.contains("[REDACTED_SECRET]"));
    assert!(!live_entries[0].message.contains("supersecretvalue"));
    assert!(
        !live_entries[0]
            .message
            .contains("abcdefghijklmnopqrstuvwxyz")
    );
}

#[test]
fn active_turn_commit_keeps_live_log_when_turn_persist_fails() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.attach_state_db(std::sync::Arc::new(state_db));
    app.snapshot.session_id = "live-persist-failure-session".to_string();

    app.push_entry(MessageRole::User, "keep me");
    app.push_entry(MessageRole::Agent, "until canonical write succeeds");
    let rollout_root = app.state_db.as_ref().unwrap().rollout_root();
    let session_dir = rollout_root.join("live-persist-failure-session");
    std::fs::create_dir(session_dir.join("turns.jsonl")).expect("turns path directory");

    app.finalize_active_turn();

    let live_entries =
        thread_turn_log::load_live_entries(&rollout_root, "live-persist-failure-session");
    assert_eq!(live_entries.len(), 2);
    assert!(app.committed_turns.is_empty());
    assert_eq!(app.active_turn.entries.len(), 2);
    assert_eq!(app.active_turn.entries[0].message, "keep me");
    assert_eq!(
        app.active_turn.entries[1].message,
        "until canonical write succeeds"
    );
    assert!(
        app.state_db_status
            .as_deref()
            .is_some_and(|status| status.contains("turn write failed"))
    );
}

#[test]
fn reset_transcript_clears_live_log() {
    let dir = tempdir().expect("tempdir");
    let state_db = StateDb::new_for_root_dir(dir.path().join(".rara")).expect("state db");
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .expect("app");
    app.attach_state_db(std::sync::Arc::new(state_db));
    app.snapshot.session_id = "live-reset-session".to_string();

    app.push_entry(MessageRole::User, "clear me");
    assert_eq!(
        thread_turn_log::load_live_entries(
            &app.state_db.as_ref().unwrap().rollout_root(),
            "live-reset-session"
        )
        .len(),
        1
    );

    app.reset_transcript();

    assert!(
        thread_turn_log::load_live_entries(
            &app.state_db.as_ref().unwrap().rollout_root(),
            "live-reset-session"
        )
        .is_empty()
    );
}

// ── Command palette selection persistence ──────────────────────────

/// Typing more characters while the palette is open should NOT reset
/// `command_palette_idx` back to 0.
#[test]
fn command_palette_selection_persists_while_typing() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette by typing slash
    app.insert_active_input_char('/');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert_eq!(app.command_palette_idx, 0);

    // Simulate arrow-down to move selection
    let cmd_count = palette_commands(&app, app.command_query()).len();
    assert!(cmd_count > 1, "need at least 2 commands for this test");
    app.command_palette_idx = 1;

    // Type more characters — this triggers sync_command_palette_with_input
    // which must NOT reset command_palette_idx when the palette is already open.
    app.insert_active_input_char('h');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert_eq!(
        app.command_palette_idx, 1,
        "selection idx should stay at 1 after typing more chars"
    );

    // Type another character — still should not reset
    app.insert_active_input_char('e');
    assert_eq!(
        app.command_palette_idx, 1,
        "selection idx should still be 1 after further typing"
    );
}

/// Closing the palette (Esc) should clear the slash input and reset
/// `command_palette_idx`.
#[test]
fn close_command_palette_clears_input_and_resets_idx() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette by typing slash
    app.insert_active_input_char('/');
    app.insert_active_input_char('h');
    app.insert_active_input_char('e');
    app.insert_active_input_char('l');
    assert!(matches!(app.overlay, Some(Overlay::CommandPalette)));
    assert!(!app.bottom_pane.input.is_empty());

    // Move selection
    app.command_palette_idx = 2;

    // Close the palette
    app.dismiss_overlay();

    // After close: input should be cleared, idx reset
    assert!(app.bottom_pane.input.is_empty(), "input should be cleared");
    assert_eq!(
        app.command_palette_idx, 0,
        "command_palette_idx should reset to 0"
    );
    assert!(app.overlay.is_none(), "overlay should be closed");
}

/// Clearing the slash prefix should close the palette and reset idx.
#[test]
fn clearing_slash_closes_palette() {
    let dir = tempdir().expect("tempdir");
    let cm = ConfigManager {
        path: dir.path().join("config.json"),
    };
    let mut app = TuiApp::new(cm).expect("app");
    app.config = RaraConfig::default();

    // Open palette and move to index 2
    app.insert_active_input_char('/');
    app.command_palette_idx = 2;

    // Backspace to clear the slash — sync fires and closes the palette
    app.backspace_active_input();
    assert!(app.overlay.is_none());
    assert_eq!(app.command_palette_idx, 0);
    assert!(app.bottom_pane.input.is_empty());
}

#[test]
fn ralph_goal_starts_pursuing_objective() {
    let goal = crate::tui::state::RalphGoal::new("run tests".into(), None);
    assert_eq!(goal.objective, "run tests");
    assert_eq!(goal.status, crate::tui::state::GoalStatus::Pursuing);
    assert_eq!(goal.tokens_used, 0);
}

#[test]
fn ralph_goal_tracks_blocked_status() {
    let mut goal = crate::tui::state::RalphGoal::new("test".into(), Some(100));
    goal.status = crate::tui::state::GoalStatus::Blocked;
    assert_eq!(goal.status, crate::tui::state::GoalStatus::Blocked);
}

#[test]
fn ralph_goal_budget_defaults_to_none() {
    let goal = crate::tui::state::RalphGoal::new("objective".into(), None);
    assert!(goal.token_budget.is_none());
    assert!(!goal.token_budget.is_some_and(|b| goal.tokens_used >= b));

    let limited = crate::tui::state::RalphGoal::new("obj".into(), Some(0));
    assert!(
        limited
            .token_budget
            .is_some_and(|b| limited.tokens_used >= b)
    );
}

#[test]
fn todo_write_emit_update_on_empty_list_clears_sidebar() {
    use serde_json::json;

    use crate::todo::normalize_todo_write_input;

    let empty = json!({"todos": []});
    let state = normalize_todo_write_input(&empty).unwrap();
    assert!(
        state.items.is_empty(),
        "empty todo_write input should produce empty state"
    );
    let view = crate::context::TodoContextView::from_state(Some(state));
    assert_eq!(view.summary.total, 0);
    assert!(view.items.is_empty());
}
