use super::*;

#[test]
fn active_turn_cell_keeps_sections_in_stable_order() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for tool output".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect the codebase".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "list_files src".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "bash cargo check".into(),
                payload: None,
            },
        ],
    };
    app.snapshot = RuntimeSnapshot {
        plan_steps: vec![("pending".into(), "Review architecture".into())],
        pending_interactions: vec![crate::tui::state::PendingInteractionSnapshot {
            kind: crate::tui::state::InteractionKind::RequestInput,
            title: "Approve plan".into(),
            summary: String::new(),
            options: vec![("1".into(), "Implement".into())],
            note: None,
            approval: None,
            source: None,
        }],
        ..RuntimeSnapshot::default()
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let you_idx = rendered.find("Inspect the codebase").unwrap();
    let exploring_idx = rendered.find("# Exploring").unwrap();
    let running_idx = rendered.find("# Running").unwrap();
    let plan_idx = rendered.find("Updated Plan").unwrap();
    let approval_idx = rendered.find("# Request Input").unwrap();

    assert!(rendered.contains("List src"));
    assert!(you_idx < running_idx);
    assert!(exploring_idx < running_idx);
    assert!(running_idx < plan_idx);
    assert!(plan_idx < approval_idx);
}

#[test]
fn active_turn_cell_renders_pending_approval_without_transcript_entries() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("resuming after approval".into());
    app.snapshot
        .pending_interactions
        .push(crate::tui::state::PendingInteractionSnapshot {
            kind: crate::tui::state::InteractionKind::Approval,
            title: "Pending Approval".into(),
            summary: "git diff origin/main -- src/context/assembler.rs".into(),
            options: Vec::new(),
            note: None,
            approval: Some(crate::tui::state::PendingApprovalSnapshot {
                tool_use_id: "toolu_123".into(),
                command: "git diff origin/main -- src/context/assembler.rs".into(),
                allow_net: false,
                payload: Default::default(),
            }),
            source: None,
        });

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Shell Approval"));
    assert!(rendered.contains("git diff origin/main -- src/context/assembler.rs"));
    assert!(!rendered.contains("resuming after approval"));
}

#[test]
fn active_turn_cell_renders_progress_sections_as_compact_stack() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect the codebase".into(),
            payload: None,
        }],
    };
    app.record_exploration_note("Inspect the auth bridge.");
    app.record_planning_note("Reuse the shared auth flow.");
    app.record_running_action("Run cargo check");

    let rendered_lines = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();

    let plan_mode_idx = rendered_lines
        .iter()
        .position(|line| line.contains("# Plan Mode"))
        .unwrap();
    let exploring_idx = rendered_lines
        .iter()
        .position(|line| line.contains("# Exploring"))
        .unwrap();
    let planning_idx = rendered_lines
        .iter()
        .position(|line| line.contains("# Planning"))
        .unwrap();
    let running_idx = rendered_lines
        .iter()
        .position(|line| line.contains("# Running"))
        .unwrap();

    assert_eq!(exploring_idx, plan_mode_idx + 2);
    assert_eq!(planning_idx, exploring_idx + 3);
    assert_eq!(running_idx, planning_idx + 3);
}

#[test]
fn active_turn_cell_hides_background_stdout_label_and_pins_stderr() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Run the formatter".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool Progress".into(),
                message: [
                    "background task stdout:",
                    "info: downloading 6 components",
                    "background task stderr:",
                    "warning: retrying download",
                    "background task stdout:",
                    "info: installing rustfmt",
                ]
                .join("\n"),
                payload: None,
            },
        ],
    };

    let rendered_lines = ActiveTurnCell::new(&app, Some(Path::new("."))).display_lines(100);
    let rendered = rendered_lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("background task stdout:"));
    assert!(!rendered.contains("background task stderr:"));
    assert!(rendered.contains("info: downloading 6 components"));
    assert!(rendered.contains("info: installing rustfmt"));
    assert!(rendered.contains("warning: retrying download"));

    let install_idx = rendered.find("info: installing rustfmt").unwrap();
    let stderr_idx = rendered.find("warning: retrying download").unwrap();
    assert!(install_idx < stderr_idx);

    let stderr_line = rendered_lines
        .iter()
        .find(|line| line.to_string().contains("warning: retrying download"))
        .expect("stderr line should render");
    assert!(
        stderr_line
            .spans
            .iter()
            .any(|span| { span.style.fg == Some(crate::tui::theme::TOOL_STDERR_FG) })
    );
}

#[test]
fn active_turn_cell_renders_terminal_result_as_terminal_cell() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Start the dev server".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "pty_start npm run dev".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool Result".into(),
                message: "pty pty-123 running: npm run dev\noutput:\nready\nlistening on 3000"
                    .into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Running pty npm run dev"));
    assert!(rendered.contains("└ ready"));
    assert!(rendered.contains("listening on 3000"));
    assert!(!rendered.contains("Run pty_start"));
}

#[test]
fn active_turn_cell_renders_typed_terminal_event_as_terminal_cell() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Start the dev server".into(),
                payload: None,
            },
            TranscriptEntry::terminal_event(TerminalEvent::End(TerminalCommandEvent {
                target: TerminalTarget::Pty,
                id: Some("pty-123".into()),
                status: "running".into(),
                command: Some("npm run dev".into()),
                exit_code: None,
                output: vec!["ready".into(), "listening on 3000".into()],
                output_path: None,
                is_error: false,
            })),
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Running pty npm run dev"));
    assert!(rendered.contains("└ ready"));
    assert!(rendered.contains("listening on 3000"));
    assert!(!rendered.contains("Terminal Event"));
}

#[test]
fn active_turn_cell_renders_planning_suggestion_without_active_turn_entries() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.queue_planning_suggestion("Review this repository and propose changes.");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Review this repository and propose changes."));
    assert!(rendered.contains("# Planning Suggested"));
    assert!(rendered.contains("Enter planning mode"));
    assert!(rendered.contains("Continue in execute mode"));
}

#[test]
fn active_turn_cell_shows_busy_feedback_without_active_turn_entries() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::SendingPrompt;
    app.runtime_phase_detail = Some("sending prompt to provider".into());

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• sending prompt to provider"));
}

#[test]
fn active_turn_cell_keeps_exploration_notes_inside_exploring_block() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for model response · 12s elapsed".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry { role: "You".into(), message: "Review this repository".into(), payload: None },
            TranscriptEntry { role: "Tool".into(), message: "read_file src/main.rs".into(), payload: None },
            TranscriptEntry {
                role: "Agent".into(),
                message:
                    "I have inspected the repository structure and will now inspect the core modules."
                        .into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("# Exploring"),
        "rendered_exploration_notes=\n{rendered}"
    );
    assert!(rendered.contains("Read src/main.rs"));
    assert!(rendered.contains(
        "I have inspected the repository structure and will now inspect the core modules."
    ));
    assert!(!rendered.contains("waiting for model response · 12s elapsed"));
}

#[test]
fn active_turn_cell_uses_stateful_live_exploration_sections() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for model response · 20s elapsed".into());
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect the repository".into(),
            payload: None,
        }],
    };
    app.record_exploration_action("Read src/tools/vector.rs");
    app.record_exploration_note("I have inspected the repository structure.");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(
        rendered.contains("# Exploring"),
        "rendered_stateful_exploration=\n{rendered}"
    );
    assert!(rendered.contains("Read src/tools/vector.rs"));
    assert!(rendered.contains("I have inspected the repository structure."));
    assert!(!rendered.contains("waiting for model response · 20s elapsed"));
}

#[test]
fn active_turn_cell_compacts_live_response_when_process_sections_exist() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for model response".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry { role: "You".into(), message: "Inspect the repository".into(), payload: None },
            TranscriptEntry { role: "Agent".into(), message: "I have inspected the repository structure.\nI checked the runtime boundary.\nI checked the prompt assembly path.\nNext I will inspect the persistence layer.\nThen I will verify the restore contract."
                    .into(), payload: None },
        ],
    };
    app.record_exploration_action("Read src/runtime_context.rs");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Exploring"));
    assert!(rendered.contains("Read src/runtime_context.rs"));
    assert!(rendered.contains("• I have inspected the repository structure."));
    assert!(rendered.contains("• Next I will inspect the persistence layer."));
    assert!(!rendered.contains("# Responding"));
    assert!(!rendered.contains("Then I will verify the restore contract."));
}

#[test]
fn active_turn_cell_appends_long_live_exploration_events() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect the repository".into(),
            payload: None,
        }],
    };
    for idx in 1..=5 {
        app.record_exploration_action(format!("Read src/module_{idx}.rs"));
    }
    app.record_exploration_note("Cross-check the auth entrypoint.");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Exploring"));
    assert!(rendered.contains("Cross-check the auth entrypoint."));
    assert!(rendered.contains("1 more exploration step(s)"));
    assert!(!rendered.contains("module_1.rs"));
    assert!(rendered.contains("module_2.rs"));
    assert!(rendered.contains("module_3.rs"));
    assert!(rendered.contains("module_4.rs"));
    assert!(rendered.contains("module_5.rs"));
    assert_eq!(rendered.matches("# Exploring").count(), 1);
    assert!(rendered.find("module_2.rs") < rendered.find("module_5.rs"));
}

#[test]
fn active_turn_cell_appends_long_live_planning_events() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Refine the plan".into(),
            payload: None,
        }],
    };
    for idx in 1..=5 {
        app.record_planning_action(format!("Inspect planning module {idx}"));
    }
    app.record_planning_note("Reuse the shared auth bridge.");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Planning"));
    assert!(rendered.contains("Reuse the shared auth bridge."));
    assert!(rendered.contains("1 more planning step(s)"));
    assert!(!rendered.contains("planning module 1"));
    assert!(rendered.contains("planning module 2"));
    assert!(rendered.contains("planning module 3"));
    assert!(rendered.contains("planning module 4"));
    assert!(rendered.contains("planning module 5"));
    assert_eq!(rendered.matches("# Planning").count(), 1);
    assert!(rendered.find("planning module 2") < rendered.find("planning module 5"));
}

#[test]
fn active_turn_cell_appends_long_live_running_events() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run the checks".into(),
            payload: None,
        }],
    };
    for idx in 1..=6 {
        app.record_running_action(format!("Run task {idx}"));
    }

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Running"));
    assert!(!rendered.contains("Run task 1"));
    assert!(!rendered.contains("Run task 2"));
    assert!(rendered.contains("Run task 3"));
    assert!(rendered.contains("Run task 6"));
    assert!(rendered.contains("more running step(s)"));
    assert_eq!(rendered.matches("# Running").count(), 1);
    assert!(rendered.find("Run task 6") < rendered.find("Run task 3"));
}

#[test]
fn active_turn_cell_updated_plan_snapshot() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model response · 3s elapsed".into());
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Read the local codebase and propose the next refactor".into(),
            payload: None,
        }],
    };
    app.record_planning_note("The auth flow should reuse codex_login instead of mirroring it.");
    app.record_exploration_action("Read src/oauth.rs");
    app.snapshot = RuntimeSnapshot {
        plan_steps: vec![
            ("completed".into(), "Inspect the current auth bridge".into()),
            (
                "in_progress".into(),
                "Replace the bespoke OAuth flow".into(),
            ),
            (
                "pending".into(),
                "Add snapshot tests for the auth picker".into(),
            ),
        ],
        plan_explanation: Some(
            "Prefer direct Codex auth reuse before extending more TUI flows.".into(),
        ),
        ..RuntimeSnapshot::default()
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_snapshot!("active_turn_cell_updated_plan", rendered);
}

#[test]
fn active_turn_cell_hides_structured_plan_response_once_plan_card_exists() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry { role: "You".into(), message: "Plan the next refactor".into(), payload: None },
            TranscriptEntry { role: "Agent".into(), message: "<proposed_plan>\n- [completed] Inspect the auth flow\n- [in_progress] Reuse codex_login\n- [pending] Add auth picker snapshots\n</proposed_plan>\nPrefer direct auth reuse before expanding more TUI flows.".into(), payload: None },
        ],
    };
    app.snapshot.plan_steps = vec![
        ("completed".into(), "Inspect the auth flow".into()),
        ("in_progress".into(), "Reuse codex_login".into()),
        ("pending".into(), "Add auth picker snapshots".into()),
    ];
    app.snapshot.plan_explanation =
        Some("Prefer direct auth reuse before expanding more TUI flows.".into());

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Updated Plan"));
    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("<proposed_plan>"));
}

#[test]
fn active_turn_cell_prefers_inline_plan_artifact_over_preamble_before_snapshot_sync() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry { role: "You".into(), message: "Review the codebase and propose changes".into(), payload: None },
            TranscriptEntry { role: "Agent".into(), message: "I reviewed the current implementation.\nHere is the concise plan.\n<proposed_plan>\n- [completed] Inspect the runtime entrypoint\n- [pending] Tighten the render path\n</proposed_plan>\nKeep the diff narrow and reviewable.".into(), payload: None },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Updated Plan"));
    assert!(rendered.contains("Inspect the runtime entrypoint"));
    assert!(rendered.contains("Keep the diff narrow and reviewable."));
    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("I reviewed the current implementation"));
    assert!(!rendered.contains("<proposed_plan>"));
}

#[test]
fn active_turn_cell_suppresses_planning_chatter_when_exploring() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message:
                    "I will now read crates/instructions/src/prompt.rs to continue the review."
                        .into(),
                payload: None,
            },
        ],
    };
    app.record_exploration_action("Read crates/instructions/src/lib.rs");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Plan Mode"));
    assert!(rendered.contains("# Exploring"));
    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("I will now read crates/instructions/src/prompt.rs"));
}

#[test]
fn active_turn_cell_uses_planning_sidecar_for_non_structured_plan_output() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Read the local codebase and suggest improvements".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Planning".into(),
                message: "The current discovery is hardcoded to root-level markdown files.".into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Planning"));
    assert!(rendered.contains("The current discovery is hardcoded"));
    assert!(!rendered.contains("Responding"));
}

#[test]
fn active_turn_cell_uses_explicit_sidecar_entries_when_live_state_is_empty() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect and summarize the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Exploring".into(),
                message: "└ Read crates/instructions/src/workspace.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Planning".into(),
                message: "The instruction discovery is still root-name based.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Running".into(),
                message: "└ waiting for model response".into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Exploring"));
    assert!(rendered.contains("# Planning"));
    assert!(rendered.contains("# Running"));
    assert!(rendered.contains("Read crates/instructions/src/workspace.rs"));
    assert!(rendered.contains("root-name based"));
    assert!(rendered.contains("waiting for model response"));
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
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "The main entrypoint is thin; I will inspect the runtime bootstrap next."
                    .into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    };

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
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "The main entrypoint is thin.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    };

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
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect the bootstrap path".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "I have narrowed this down to the runtime bootstrap path.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool".into(),
                message: "read_file src/runtime_context.rs".into(),
                payload: None,
            },
        ],
    };

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
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Continue the migration".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "First I will sync the branch.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Running".into(),
                message: "Run git rebase main".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "The first conflict is in keymap.rs.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Running".into(),
                message: "Run cargo test tui::keymap".into(),
                payload: None,
            },
        ],
    };

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
fn active_turn_cell_uses_lightweight_busy_response_when_not_streaming() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model response · 2s elapsed".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "I have inspected the main module and will continue with the tool layer."
                    .into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("╭"));
    assert!(!rendered.contains("╰"));
    assert!(rendered.contains("• I have inspected"));
    assert!(!rendered.contains("Agent:"));
}

#[test]
fn active_turn_cell_shows_live_thinking_stream() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("thinking".into());
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review this repository".into(),
            payload: None,
        }],
    };
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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run a long task".into(),
            payload: None,
        }],
    };

    app.append_agent_thinking_delta("first reasoning block\n");
    app.flush_agent_thinking_stream_to_live_event();
    app.record_running_action("Run cargo check");
    app.append_agent_thinking_delta("second reasoning block\n");
    app.flush_agent_thinking_stream_to_live_event();
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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run a long task".into(),
            payload: None,
        }],
    };

    app.append_agent_thinking_delta("first reasoning block\n");
    app.flush_agent_thinking_stream_to_live_event();
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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect before reasoning again".into(),
            payload: None,
        }],
    };

    app.append_agent_thinking_delta("first reasoning block\n");
    app.flush_agent_thinking_stream_to_live_event();
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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Reason about a task".into(),
            payload: None,
        }],
    };

    app.append_agent_thinking_delta("first reasoning block\n");
    app.flush_agent_thinking_stream_to_live_event();
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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect thinking formatting".into(),
            payload: None,
        }],
    };

    app.append_agent_thinking_delta("    let value = 1;\n");
    app.flush_agent_thinking_stream_to_live_event();

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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run checks".into(),
            payload: None,
        }],
    };

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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Inspect and run checks".into(),
            payload: None,
        }],
    };

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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run checks".into(),
            payload: None,
        }],
    };

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
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review this repository".into(),
            payload: None,
        }],
    };
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
fn active_turn_cell_renders_live_response_as_lightweight_message() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model response".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Agent".into(),
                message: "I have inspected the main module and will continue with the tool layer."
                    .into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("╭"));
    assert!(!rendered.contains("╰"));
    assert!(rendered.contains("• I have inspected the main module"));
}

#[test]
fn active_turn_cell_prefers_responding_over_tool_result_while_processing_response() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model output".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Review the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool Result".into(),
                message: "bash stdout: partial output".into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• waiting for model output"));
    assert!(!rendered.contains("Tool Result"));
    assert!(!rendered.contains("bash stdout: partial output"));
}

#[test]
fn active_turn_cell_prefers_responding_over_system_notice_while_sending_prompt() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::SendingPrompt;
    app.runtime_phase_detail = Some("sending prompt to provider".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Review the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "System".into(),
                message: "temporary setup notice".into(),
                payload: None,
            },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• sending prompt to provider"));
    assert!(!rendered.contains("temporary setup notice"));
}

#[test]
fn active_turn_cell_shows_planning_section_for_plan_agent() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("plan_agent {\"instruction\":\"refine the plan\"}".into());
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Plan the refactor".into(),
            payload: None,
        }],
    };
    app.record_planning_action("Delegate plan refinement: refine the plan");
    app.record_planning_note("Sub-agent summary: reuse the workspace traversal helper");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Planning"));
    assert!(rendered.contains("Delegate plan refinement: refine the plan"));
    assert!(rendered.contains("Sub-agent summary: reuse the workspace traversal helper"));
}

#[test]
fn active_turn_cell_renders_device_code_prompt_system_message() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::OAuthPollingDeviceCode;
    app.runtime_phase_detail = Some("Waiting for device-code confirmation.".into());
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry { role: "Runtime".into(), message: "Starting Codex device-code login flow.".into(), payload: None },
            TranscriptEntry { role: "System".into(), message: "Open this URL in a browser and enter the one-time code:\nhttps://example.test\n\nCode: ABCD".into(), payload: Some(crate::tui::state::TranscriptEntryPayload::System(crate::tui::state::SystemMessageKind::OAuthPrompt)) },
        ],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("System"));
    assert!(rendered.contains("https://example.test"));
    assert!(rendered.contains("Code: ABCD"));
}
