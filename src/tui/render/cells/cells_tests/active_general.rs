use super::*;
use crate::tui::message_role::MessageRole;

#[test]
fn active_turn_cell_keeps_sections_in_stable_order() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("waiting for tool output".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Inspect the codebase".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "list_files src".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "bash cargo check".into(),
                payload: None,
            },
        ],
    }
    .into();
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
            created_at_epoch_seconds: None,
        }],
        ..RuntimeSnapshot::default()
    }
    .into();

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
    app.runtime_phase_detail = Some("resuming after approval".into()).into();
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
            created_at_epoch_seconds: None,
        });

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("# Shell Approval"));
    assert!(rendered.contains("git diff origin/main -- src/context/assembler.rs"));
    assert!(rendered.contains("Working directory:"));
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
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Inspect the codebase".into(),
            payload: None,
        }],
    }
    .into();
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
fn active_turn_cell_renders_planning_prompt_with_recorded_notice() {
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
    app.runtime_phase_detail = Some("sending prompt to provider".into()).into();

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
    app.runtime_phase_detail = Some("waiting for model response · 12s elapsed".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry { role: MessageRole::User, message: "Review this repository".into(), payload: None },
            TranscriptEntry { role: MessageRole::Tool, message: "read_file src/main.rs".into(), payload: None },
            TranscriptEntry {
                role: MessageRole::Agent,
                message:
                    "I have inspected the repository structure and will now inspect the core modules."
                        .into(),
                payload: None,
            },
        ],
    }.into();

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
    app.runtime_phase_detail = Some("waiting for model response · 20s elapsed".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Inspect the repository".into(),
            payload: None,
        }],
    }
    .into();
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
fn active_turn_cell_appends_long_live_exploration_events() {
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
            message: "Inspect the repository".into(),
            payload: None,
        }],
    }
    .into();
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
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Refine the plan".into(),
            payload: None,
        }],
    }
    .into();
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
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Run the checks".into(),
            payload: None,
        }],
    }
    .into();
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
    app.runtime_phase_detail = Some("waiting for model response · 3s elapsed".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Read the local codebase and propose the next refactor".into(),
            payload: None,
        }],
    }
    .into();
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
    }
    .into();

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
        thinking_duration: None,
        entries: vec![
            TranscriptEntry { role: MessageRole::User, message: "Plan the next refactor".into(), payload: None },
            TranscriptEntry { role: MessageRole::Agent, message: "<proposed_plan>\n- [completed] Inspect the auth flow\n- [in_progress] Reuse codex_login\n- [pending] Add auth picker snapshots\n</proposed_plan>\nPrefer direct auth reuse before expanding more TUI flows.".into(), payload: None },
        ],
    }.into();
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
        thinking_duration: None,
        entries: vec![
            TranscriptEntry { role: MessageRole::User, message: "Review the codebase and propose changes".into(), payload: None },
            TranscriptEntry { role: MessageRole::Agent, message: "I reviewed the current implementation.\nHere is the concise plan.\n<proposed_plan>\n- [completed] Inspect the runtime entrypoint\n- [pending] Tighten the render path\n</proposed_plan>\nKeep the diff narrow and reviewable.".into(), payload: None },
        ],
    }.into();

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
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message:
                    "I will now read crates/instructions/src/prompt.rs to continue the review."
                        .into(),
                payload: None,
            },
        ],
    }
    .into();
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
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Read the local codebase and suggest improvements".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Planning,
                message: "The current discovery is hardcoded to root-level markdown files.".into(),
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
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Inspect and summarize the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Exploring,
                message: "└ Read crates/instructions/src/workspace.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Planning,
                message: "The instruction discovery is still root-name based.".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Running,
                message: "└ waiting for model response".into(),
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

    assert!(rendered.contains("# Exploring"));
    assert!(rendered.contains("# Planning"));
    assert!(rendered.contains("# Running"));
    assert!(rendered.contains("Read crates/instructions/src/workspace.rs"));
    assert!(rendered.contains("root-name based"));
    assert!(rendered.contains("waiting for model response"));
}

#[test]
fn active_turn_cell_uses_lightweight_busy_response_when_not_streaming() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model response · 2s elapsed".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "I have inspected the main module and will continue with the tool layer."
                    .into(),
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

    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("╭"));
    assert!(!rendered.contains("╰"));
    assert!(rendered.contains("• I have inspected"));
    assert!(!rendered.contains("Agent:"));
}

#[test]
fn active_turn_cell_renders_live_response_as_lightweight_message() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model response".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review this repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Agent,
                message: "I have inspected the main module and will continue with the tool layer."
                    .into(),
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

    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("╭"));
    assert!(!rendered.contains("╰"));
    assert!(rendered.contains("• I have inspected the main module"));
}

#[test]
fn active_turn_cell_prefers_responding_over_system_notice_while_sending_prompt() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::SendingPrompt;
    app.runtime_phase_detail = Some("sending prompt to provider".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::System,
                message: "temporary setup notice".into(),
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
    app.runtime_phase_detail =
        Some("plan_agent {\"instruction\":\"refine the plan\"}".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry {
            role: MessageRole::User,
            message: "Plan the refactor".into(),
            payload: None,
        }],
    }
    .into();
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
    app.runtime_phase_detail = Some("Waiting for device-code confirmation.".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry { role: MessageRole::Runtime, message: "Starting Codex device-code login flow.".into(), payload: None },
            TranscriptEntry { role: MessageRole::System, message: "Open this URL in a browser and enter the one-time code:\nhttps://example.test\n\nCode: ABCD".into(), payload: Some(crate::tui::state::TranscriptEntryPayload::System(crate::tui::state::SystemMessageKind::OAuthPrompt)) },
        ],
    }.into();

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
