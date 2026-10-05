use super::*;
use crate::tui::message_role::MessageRole;

#[test]
fn active_turn_cell_keeps_open_thinking_after_prior_agent_segment() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.push_entry(MessageRole::User, "Explain the ordering");
    app.append_agent_delta("First response segment.");
    app.append_agent_thinking_delta("Later reasoning segment.");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let agent_idx = rendered.find("First response segment.").unwrap();
    let thinking_idx = rendered.find("┊ Thinking").unwrap();
    assert!(agent_idx < thinking_idx);
}

#[test]
fn active_turn_cell_preserves_agent_before_later_live_progress() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for model response".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry { role: MessageRole::User, message: "Inspect the repository".into(), payload: None },
            TranscriptEntry { role: MessageRole::Agent, message: "I have inspected the repository structure.\nI checked the runtime boundary.\nI checked the prompt assembly path.\nNext I will inspect the persistence layer.\nThen I will verify the restore contract."
                    .into(), payload: None },
        ],
    }.into();
    app.record_exploration_action("Read src/runtime_context.rs");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let agent_idx = rendered
        .find("• I have inspected the repository structure.")
        .unwrap();
    let exploring_idx = rendered.find("# Exploring").unwrap();

    assert!(agent_idx < exploring_idx);
    assert!(rendered.contains("Read src/runtime_context.rs"));
    assert!(rendered.contains("Next I will inspect the persistence layer."));
    assert!(!rendered.contains("# Responding"));
    assert!(rendered.contains("Then I will verify the restore contract."));
}

#[test]
fn active_turn_cell_preserves_exploration_agent_exploration_order() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Inspect the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "The main entrypoint is thin; I will inspect the runtime bootstrap next."
                    .into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_exploring = rendered.find("# Exploring").unwrap();
    let agent = rendered
        .find("• The main entrypoint is thin; I will inspect the runtime bootstrap next.")
        .unwrap();
    let second_exploring = rendered[first_exploring + 1..]
        .find("# Exploring")
        .map(|idx| first_exploring + 1 + idx)
        .unwrap();

    assert!(rendered.contains("Read src/main.rs"));
    assert!(rendered.contains("Read src/runtime_context.rs"));
    assert!(first_exploring < agent);
    assert!(agent < second_exploring);
}

#[test]
fn active_turn_cell_preserves_duplicate_restored_exploration_segments() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Inspect the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "The main entrypoint is thin.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("Read src/main.rs").count(), 2);
    assert_eq!(rendered.matches("Read src/runtime_context.rs").count(), 2);
    assert_eq!(rendered.matches("# Exploring").count(), 2);
}

#[test]
fn active_turn_cell_preserves_agent_then_exploration_order() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Inspect the bootstrap path".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "I have narrowed this down to the runtime bootstrap path.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let agent = rendered
        .find("• I have narrowed this down to the runtime bootstrap path.")
        .unwrap();
    let exploring = rendered.find("# Exploring").unwrap();

    assert!(rendered.contains("Read src/runtime_context.rs"));
    assert!(agent < exploring);
}

#[test]
fn active_turn_cell_preserves_interleaved_agent_and_progress_output() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Continue the migration".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "First I will sync the branch.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Running,
                message: "Run git rebase main".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "The first conflict is in keymap.rs.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Running,
                message: "Run cargo test tui::keymap".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_agent = rendered.find("First I will sync the branch.").unwrap();
    let first_running = rendered.find("Run git rebase main").unwrap();
    let second_agent = rendered
        .find("The first conflict is in keymap.rs.")
        .unwrap();
    let second_running = rendered.find("Run cargo test tui::keymap").unwrap();

    assert!(first_agent < first_running);
    assert!(first_running < second_agent);
    assert!(second_agent < second_running);
    assert_eq!(
        rendered.matches("• First I will sync the branch.").count(),
        1
    );
    assert_eq!(
        rendered
            .matches("• The first conflict is in keymap.rs.")
            .count(),
        1
    );
    assert_eq!(rendered.matches("Run git rebase main").count(), 1);
    assert_eq!(rendered.matches("Run cargo test tui::keymap").count(), 1);
}

#[test]
fn active_turn_cell_shows_live_thinking_stream() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("thinking".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Review this repository".into(),
            payload: None,
        }],
    }
    .into();
    app.append_agent_thinking_delta("checking runtime events\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("┊ checking runtime events"));
}

#[test]
fn active_turn_cell_flattens_thinking_and_running_events_in_order() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Run a long task".into(),
            payload: None,
        }],
    }
    .into();

    app.append_agent_thinking_delta("first reasoning block\n");
    app.finalize_agent_thinking_stream();
    app.record_running_action("Run cargo check");
    app.append_agent_thinking_delta("second reasoning block\n");
    app.finalize_agent_thinking_stream();
    app.record_running_action("Run cargo test");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_thinking = rendered.find("first reasoning block").unwrap();
    let first_running = rendered.find("Run cargo check").unwrap();
    let second_thinking = rendered.find("second reasoning block").unwrap();
    let second_running = rendered.find("Run cargo test").unwrap();

    assert!(first_thinking < first_running);
    assert!(first_running < second_thinking);
    assert!(second_thinking < second_running);
    assert!(rendered.contains("┊ first reasoning block"));
    assert!(rendered.contains("┊ second reasoning block"));
    assert_eq!(rendered.matches("# Running").count(), 2);
}

#[test]
fn active_turn_cell_places_streaming_thinking_after_latest_progress_event() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Run a long task".into(),
            payload: None,
        }],
    }
    .into();

    app.append_agent_thinking_delta("first reasoning block\n");
    app.finalize_agent_thinking_stream();
    app.record_running_action("Run cargo check");
    app.append_agent_thinking_delta("second reasoning block\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_thinking = rendered.find("first reasoning block").unwrap();
    let running = rendered.find("Run cargo check").unwrap();
    let second_thinking = rendered.find("second reasoning block").unwrap();

    assert!(first_thinking < running);
    assert!(running < second_thinking);
    assert!(rendered.contains("┊ first reasoning block"));
    assert!(rendered.contains("┊ second reasoning block"));
    assert_eq!(rendered.matches("# Running").count(), 1);
}

#[test]
fn active_turn_cell_places_streaming_thinking_after_latest_exploration_event() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Inspect before reasoning again".into(),
            payload: None,
        }],
    }
    .into();

    app.append_agent_thinking_delta("first reasoning block\n");
    app.finalize_agent_thinking_stream();
    app.record_exploration_action("Read src/tui/render/cells.rs");
    app.append_agent_thinking_delta("second reasoning block\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_thinking = rendered.find("first reasoning block").unwrap();
    let exploring = rendered.find("Read src/tui/render/cells.rs").unwrap();
    let second_thinking = rendered.find("second reasoning block").unwrap();

    assert!(first_thinking < exploring);
    assert!(exploring < second_thinking);
    assert!(rendered.contains("┊ first reasoning block"));
    assert!(rendered.contains("┊ second reasoning block"));
    assert_eq!(rendered.matches("# Exploring").count(), 1);
}

#[test]
fn active_turn_cell_groups_consecutive_thinking_events_with_stream() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Reason about a task".into(),
            payload: None,
        }],
    }
    .into();

    app.append_agent_thinking_delta("first reasoning block\n");
    app.finalize_agent_thinking_stream();
    app.append_agent_thinking_delta("second reasoning block\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_thinking = rendered.find("first reasoning block").unwrap();
    let second_thinking = rendered.find("second reasoning block").unwrap();

    assert!(first_thinking < second_thinking);
    assert!(rendered.contains("┊ first reasoning block"));
}

#[test]
fn active_turn_cell_preserves_flushed_thinking_leading_indentation() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Inspect thinking formatting".into(),
            payload: None,
        }],
    }
    .into();

    app.append_agent_thinking_delta("    let value = 1;\n");
    app.finalize_agent_thinking_stream();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("┊     let value = 1;"));
}

#[test]
fn active_turn_cell_preserves_repeated_progress_events_when_interleaved() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Run checks".into(),
            payload: None,
        }],
    }
    .into();

    app.record_running_action("Run cargo check");
    app.record_planning_note("Inspect the next failure before retrying.");
    app.record_running_action("Run cargo check");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_running_idx = rendered.find("Run cargo check").unwrap();
    let planning_idx = rendered
        .find("Inspect the next failure before retrying.")
        .unwrap();
    let second_running_idx = rendered.rfind("Run cargo check").unwrap();

    assert!(first_running_idx < planning_idx);
    assert!(planning_idx < second_running_idx);
}

#[test]
fn active_turn_cell_groups_consecutive_exploration_events_only() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Inspect and run checks".into(),
            payload: None,
        }],
    }
    .into();

    app.record_exploration_action("Read src/tui/render/cells.rs");
    app.record_exploration_action("Read src/tui/render/cells_tests/active_general.rs");
    app.record_running_action("Run cargo check");
    app.record_exploration_action("Read src/tui/render/cells_components.rs");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("# Exploring").count(), 2);

    let first_exploring_idx = rendered.find("# Exploring").unwrap();
    let first_read_idx = rendered.find("Read src/tui/render/cells.rs").unwrap();
    let second_read_idx = rendered
        .find("Read src/tui/render/cells_tests/active_general.rs")
        .unwrap();
    let running_idx = rendered.find("# Running").unwrap();
    let second_exploring_idx = rendered.rfind("# Exploring").unwrap();
    let third_read_idx = rendered
        .find("Read src/tui/render/cells_components.rs")
        .unwrap();

    assert!(first_exploring_idx < first_read_idx);
    assert!(first_read_idx < second_read_idx);
    assert!(second_read_idx < running_idx);
    assert!(running_idx < second_exploring_idx);
    assert!(second_exploring_idx < third_read_idx);
}

#[test]
fn active_turn_cell_preserves_consecutive_duplicate_progress_events() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Run checks".into(),
            payload: None,
        }],
    }
    .into();

    app.record_exploration_action("Read src/tui/render/cells.rs");
    app.record_exploration_action("Read src/tui/render/cells.rs");
    app.record_running_action("Run cargo check");
    app.record_running_action("Run cargo check");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(rendered.matches("Read src/tui/render/cells.rs").count(), 2);
    assert_eq!(rendered.matches("Run cargo check").count(), 2);
    assert_eq!(rendered.matches("# Exploring").count(), 1);
    assert_eq!(rendered.matches("# Running").count(), 1);
}

#[test]
fn active_turn_cell_shows_live_thinking_tail_without_cloning_full_body() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Review this repository".into(),
            payload: None,
        }],
    }
    .into();
    app.append_agent_thinking_delta("line 1\nline 2\nline 3\nline 4\nline 5\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("┊  ... 1 more lines"));
    assert!(!rendered.contains("┊ line 1"));
    assert!(rendered.contains("┊ line 2"));
    assert!(rendered.contains("┊ line 5"));
}

#[test]
fn active_turn_cell_hides_successful_bash_result_while_thinking() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("thinking".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: "bash finished with exit code 0".into(),
                payload: None,
            },
        ],
    }
    .into();
    app.append_agent_thinking_delta("checking the result\n");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Thinking"));
    assert!(rendered.contains("checking the result"));
    assert!(!rendered.contains("bash finished with exit code 0"));
    assert!(!rendered.contains("✓ bash"));
}
