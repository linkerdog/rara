use super::*;

#[test]
fn active_turn_cell_renders_plan_approval_as_interaction_card() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review the codebase and propose changes".into(),
            payload: None,
        }],
    };
    app.snapshot.plan_steps = vec![
        ("pending".into(), "Generalize instruction discovery".into()),
        (
            "pending".into(),
            "Preserve cache and path resolution behavior".into(),
        ),
    ];
    app.snapshot.plan_explanation =
        Some("The current discovery path is hardcoded and should be generalized.".into());
    app.set_pending_plan_approval(true);

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert_snapshot!("active_turn_cell_plan_approval", rendered);
    assert!(rendered.contains(" Awaiting Approval "));
    assert!(rendered.contains("Updated Plan"));
    assert!(rendered.contains("1. Start implementation now"));
    assert!(rendered.contains("2. Continue planning and refine the plan"));
    assert!(rendered.contains("Generalize instruction discovery"));
}

#[test]
fn active_turn_cell_renders_updated_plan_checklist() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.agent_execution_mode = crate::agent::AgentExecutionMode::Plan;
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Improve the plan rendering".into(),
            payload: None,
        }],
    };
    app.snapshot.plan_steps = vec![
        ("completed".into(), "Inspect the current plan UI".into()),
        (
            "in_progress".into(),
            "Introduce a dedicated plan formatter".into(),
        ),
        (
            "pending".into(),
            "Unify status and transcript rendering".into(),
        ),
    ];
    app.snapshot.plan_explanation =
        Some("Keep the plan display aligned with Codex checklist semantics.".into());

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Updated Plan"));
    assert!(rendered.contains("Keep the plan display aligned with Codex checklist semantics."));
    assert!(rendered.contains("✔ Inspect the current plan UI"));
    assert!(rendered.contains("□ Introduce a dedicated plan formatter"));
    assert!(rendered.contains("□ Unify status and transcript rendering"));
}

#[test]
fn active_turn_cell_hides_stale_updated_plan_after_plan_turn_finishes() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Implement the approved fix".into(),
            payload: None,
        }],
    };
    app.snapshot.plan_steps = vec![("pending".into(), "Inspect the config loading flow".into())];
    app.snapshot.plan_explanation = Some("This should not keep rendering after plan exit.".into());

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("Updated Plan"));
    assert!(rendered.contains("Implement the approved fix"));
}

#[test]
fn active_turn_cell_hides_stale_exploring_after_live_phase_finishes() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::Idle;
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Inspect the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Exploring".into(),
                message: "└ Read src/main.rs".into(),
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

    assert!(!rendered.contains(" Exploring "));
    assert!(rendered.contains("Inspect the repository"));
}

#[test]
fn active_turn_cell_renders_shell_approval_as_interaction_card() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Run the migration helper".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool Progress".into(),
                message: "background task stdout:\n    Checking nix v0.30.1".into(),
                payload: None,
            },
        ],
    };
    app.snapshot
        .pending_interactions
        .push(crate::tui::state::PendingInteractionSnapshot {
            kind: crate::tui::state::InteractionKind::Approval,
            title: "Pending Approval".into(),
            summary: "bash ./scripts/migrate.sh".into(),
            options: Vec::new(),
            note: None,
            approval: Some(crate::tui::state::PendingApprovalSnapshot {
                tool_use_id: "toolu_123".into(),
                command: "bash ./scripts/migrate.sh".into(),
                allow_net: false,
                payload: crate::tools::bash::BashCommandInput {
                    command: None,
                    program: Some("bash".into()),
                    args: vec!["./scripts/migrate.sh".into()],
                    cwd: Some("/repo".into()),
                    env: Default::default(),
                    allow_net: false,
                    run_in_background: false,
                    ..Default::default()
                },
            }),
            source: None,
        });

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" Shell Approval "));
    assert!(rendered.contains("Review this shell command before RARA runs it."));
    assert!(rendered.contains("Command:"));
    assert!(rendered.contains("Working directory:"));
    assert!(rendered.contains("bash ./scripts/migrate.sh"));
    assert!(rendered.contains("1. Allow once"));
    assert!(!rendered.contains("Tool Progress"));
    assert!(!rendered.contains("background task stdout"));
}

#[test]
fn active_turn_cell_renders_queued_follow_up_without_hiding_shell_approval() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Run a shell command".into(),
            payload: None,
        }],
    };
    app.queue_follow_up_message("then review the diff");
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

    assert!(rendered.contains(" Shell Approval "));
    assert!(rendered.contains("1. Allow once"));
    assert!(rendered.contains(" Queued "));
    assert!(rendered.contains("after turn"));
    assert!(!rendered.contains("Queued follow-up messages"));
    assert!(rendered.contains("then review the diff"));
}

#[test]
fn active_turn_cell_does_not_render_completed_plan_decision() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review the codebase and propose changes".into(),
            payload: None,
        }],
    };
    app.record_completed_interaction(
        crate::tui::state::InteractionKind::PlanApproval,
        "Plan Decision",
        "Approved and started implementation",
        None,
    );

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains(" Plan Decision "));
    assert!(!rendered.contains("Approved and started implementation"));
}

#[test]
fn active_turn_cell_does_not_render_completed_shell_approval_while_live() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Run the migration helper".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Shell Approval Completed".into(),
                message: "Bash approval: Approved once for command: bash ./scripts/migrate.sh"
                    .into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Running".into(),
                message: "bash ./scripts/migrate.sh".into(),
                payload: None,
            },
        ],
    };
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("resuming after approval".into()),
    );

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains(" Shell Approval Completed "));
    assert!(!rendered.contains("Approved once for command"));
    assert!(rendered.contains(" Running "));
    assert!(rendered.contains("bash ./scripts/migrate.sh"));
}

#[test]
fn active_turn_cell_falls_back_to_previous_completion_when_shell_approval_is_live() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "Answer and run".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Question Answered".into(),
                message: "User answered: yes".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Shell Approval Completed".into(),
                message: "Bash approval: Approved once for command: cargo check".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Running".into(),
                message: "cargo check".into(),
                payload: None,
            },
        ],
    };
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("resuming after approval".into()),
    );

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" Question Answered "));
    assert!(rendered.contains("User answered: yes"));
    assert!(!rendered.contains(" Shell Approval Completed "));
    assert!(!rendered.contains("Approved once for command"));
}

#[test]
fn active_turn_cell_keeps_streaming_response_without_responding_card() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![
            TranscriptEntry {
                role: "You".into(),
                message: "你好".into(),
                payload: None,
            },
            TranscriptEntry {
                role: "Tool Result".into(),
                message: "bash stdout: partial".into(),
                payload: None,
            },
        ],
    };
    app.set_runtime_phase(
        RuntimePhase::ProcessingResponse,
        Some("streaming model output".into()),
    );
    app.append_agent_delta("你好");

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("你好"));
    assert!(!rendered.contains("Responding"));
    assert!(!rendered.contains("╭"));
    assert!(!rendered.contains("╰"));
    assert!(!rendered.contains("Working"));
}

#[test]
fn active_turn_cell_does_not_repeat_stale_plan_decision_from_snapshot() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Approve the plan".into(),
            payload: None,
        }],
    };
    app.record_completed_interaction(
        crate::tui::state::InteractionKind::PlanApproval,
        "Plan Decision",
        "Approved and started implementation",
        None,
    );
    app.finalize_active_turn();
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Continue with the next task".into(),
            payload: None,
        }],
    };

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains(" Plan Decision "));
}

#[test]
fn active_turn_cell_labels_delegated_plan_questions() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review the codebase and propose changes".into(),
            payload: None,
        }],
    };
    app.record_local_request_input(
        "plan_agent",
        "Which discovery strategy should we keep?",
        vec![
            ("Minimal".into(), "Keep root-only files.".into()),
            (
                "Generic".into(),
                "Scan all instruction markdown files.".into(),
            ),
        ],
        Some("A product decision is needed before editing.".into()),
    );

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" Planning Question "));
    assert!(rendered.contains("from:"));
    assert!(rendered.contains("plan_agent"));
    assert!(rendered.contains("Which discovery strategy should we keep?"));
}

#[test]
fn active_turn_cell_labels_delegated_completed_questions() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.active_turn = TranscriptTurn {
        entries: vec![TranscriptEntry {
            role: "You".into(),
            message: "Review the codebase and propose changes".into(),
            payload: None,
        }],
    };
    app.record_completed_interaction(
        crate::tui::state::InteractionKind::RequestInput,
        "Which discovery strategy should we keep?",
        "Answered with: Generic",
        Some("plan_agent".into()),
    );

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains(" Planning Question Answered "));
    assert!(rendered.contains("Answered with: Generic"));
}
