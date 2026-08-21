#[tokio::test]
async fn enter_plan_mode_tool_switches_to_read_only_planning() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "enter-plan".to_string(),
                name: "enter_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "The main issue is that planning and approval are coupled.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(EnterPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "review the planning implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should enter planning mode and return analysis");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.last_query_produced_plan());
    assert!(agent.current_plan.is_empty());

    let observed_tools = backend.observed_tools();
    assert_eq!(observed_tools.len(), 2);
    assert!(observed_tools[0].contains(&"enter_plan_mode".to_string()));
    assert!(!observed_tools[1].contains(&"enter_plan_mode".to_string()));
}

#[tokio::test]
async fn enter_plan_mode_prevents_earlier_mutating_tool_in_same_batch() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::ToolUse {
                    id: "write-before-plan".to_string(),
                    name: "bash".to_string(),
                    input: json!({ "command": "git push origin main" }),
                },
                ContentBlock::ToolUse {
                    id: "enter-plan".to_string(),
                    name: "enter_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will inspect first.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
    tool_manager.register(Box::new(EnterPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode(
            "review then maybe implement".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should enter plan mode before executing batch tools");

    let history = agent
        .history
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(history.contains("bash is read-only in plan mode"));
    assert!(!history.contains("\"stdout\":\"ok"));
}

#[tokio::test]
async fn exit_plan_mode_persists_plan_and_waits_for_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state\n- [pending] Add regression coverage\n</proposed_plan>".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Implemented the approved plan.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager.clone(),
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "plan the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should stop at exit plan approval");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.has_pending_plan_exit_approval());
    let plan_file = session_manager.plan_file_path(&agent.session_id);
    let plan = std::fs::read_to_string(plan_file).expect("plan file should be persisted");
    assert!(plan.contains("- [pending] Update planning state"));
    assert!(plan.contains("- [pending] Add regression coverage"));

    agent
        .resume_after_plan_approval_with_events(
            false,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("approved plan should resume execution");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Execute);
    assert!(!agent.has_pending_plan_exit_approval());
    let history = agent
        .history
        .iter()
        .map(|message| message.content.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(history.contains("User has approved your plan. You can now start coding."));
    assert!(history.contains("Approved Plan"));
    assert!(history.contains("Implemented the approved plan."));
}

#[tokio::test]
async fn exit_plan_mode_accepts_structured_tool_plan_input() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "exit-plan".to_string(),
            name: "exit_plan_mode".to_string(),
            input: json!({
                "proposed_plan": {
                    "summary": "Repair malformed plan exits.",
                    "steps": [
                        { "step": "Capture proposed_plan from tool arguments", "status": "pending" },
                        { "step": "Persist the structured plan before approval", "status": "pending" }
                    ],
                    "validation": [
                        "cargo test exit_plan_mode -- --nocapture"
                    ]
                }
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager.clone(),
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "plan the implementation".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("structured tool plan should stop at plan approval");

    assert!(agent.has_pending_plan_exit_approval());
    assert_eq!(agent.current_plan.len(), 2);
    assert_eq!(
        agent.current_plan[0].step,
        "Capture proposed_plan from tool arguments"
    );
    assert_eq!(
        agent.plan_explanation.as_deref(),
        Some(
            "summary: Repair malformed plan exits.\nvalidation:\n- cargo test exit_plan_mode -- --nocapture"
        )
    );
    let plan_file = session_manager.plan_file_path(&agent.session_id);
    let plan = std::fs::read_to_string(plan_file).expect("plan file should be persisted");
    assert!(plan.contains("summary: Repair malformed plan exits."));
    assert!(plan.contains("- [pending] Capture proposed_plan from tool arguments"));
    assert!(plan.contains("cargo test exit_plan_mode -- --nocapture"));
}

#[tokio::test]
async fn exit_plan_mode_without_plan_gets_one_structured_repair_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-invalid".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Fix plan exit handling\n</proposed_plan>"
                        .to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-valid".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    agent
        .query_with_mode(
            "exit without a concrete plan".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("invalid plan exit should get one repair turn");

    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    assert!(observed_messages[1].iter().any(|message| {
        let content = message.content.to_string();
        content.contains("plan_exit_repair_required")
            && content.contains("Markdown headings")
            && content.contains("<proposed_plan>")
    }));
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.has_pending_plan_exit_approval());
    assert!(agent.current_plan.iter().any(|step| {
        step.step == "Fix plan exit handling" && matches!(step.status, PlanStepStatus::Pending)
    }));
}

#[tokio::test]
async fn exit_plan_mode_requires_plan_from_same_assistant_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-first".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "exit-plan-second".to_string(),
                name: "exit_plan_mode".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent.current_plan = vec![PlanStep {
        step: "stale plan step".to_string(),
        status: PlanStepStatus::Pending,
    }];

    let mut events = Vec::new();
    agent
        .query_with_mode_and_events(
            "exit with stale plan only".to_string(),
            super::super::AgentOutputMode::Silent,
            |event| events.push(event),
        )
        .await
        .expect("stale plan exit should get one repair attempt before stopping");

    assert_eq!(backend.observed_messages().len(), 2);
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.has_pending_plan_exit_approval());
    let repair_status_seen = events.iter().any(|event| {
        matches!(
            event,
            AgentEvent::Status(status)
                if status.contains("missing a structured proposed plan")
        )
    });
    assert!(repair_status_seen);
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            name,
            content,
            is_error: true,
            ..
        } if name == "exit_plan_mode"
            && content.contains("exit_plan_mode requires a proposed plan")
    )));
}

#[tokio::test]
async fn exit_plan_mode_with_unclosed_proposed_plan_reports_specific_error() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-first".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![
                ContentBlock::Text {
                    text: "<proposed_plan>\n- [pending] Update planning state".to_string(),
                },
                ContentBlock::ToolUse {
                    id: "exit-plan-second".to_string(),
                    name: "exit_plan_mode".to_string(),
                    input: json!({}),
                },
            ],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(ExitPlanModeTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);

    let mut events = Vec::new();
    agent
        .query_with_mode_and_events(
            "exit with an incomplete plan".to_string(),
            super::super::AgentOutputMode::Silent,
            |event| events.push(event),
        )
        .await
        .expect("invalid plan exit should get one repair attempt before stopping");

    assert_eq!(backend.observed_messages().len(), 2);
    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(!agent.has_pending_plan_exit_approval());
    assert!(agent.current_plan.is_empty());
    assert!(events.iter().any(|event| matches!(
        event,
        AgentEvent::ToolResult {
            name,
            content,
            is_error: true,
            ..
        } if name == "exit_plan_mode"
            && content.contains("complete <proposed_plan>...</proposed_plan> block")
            && content.contains("</proposed_plan>")
    )));
}

#[tokio::test]
async fn continues_tool_loop_without_fixed_turn_cap() {
    let tool_turns = 205;
    let mut responses = (0..tool_turns)
        .map(|idx| LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: format!("tool-{idx}"),
                name: "stub_tool".to_string(),
                input: json!({}),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        })
        .collect::<Vec<_>>();
    responses.push(LlmResponse {
        content: vec![ContentBlock::Text {
            text: "Final answer after reviewing the tool results.".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    });
    let backend = Arc::new(SequencedBackend::new(responses));

    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubTool));
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        tool_manager,
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode("loop".to_string(), super::super::AgentOutputMode::Silent)
        .await
        .expect("query should continue until the model returns a final answer");

    let observed_tools = backend.observed_tools();
    assert_eq!(
        observed_tools.len(),
        tool_turns + 1,
        "the agent should continue past the former fixed turn cap before the final answer"
    );
    assert!(agent.history.last().is_some_and(|message| {
        message
            .content
            .to_string()
            .contains("Final answer after reviewing the tool results.")
    }));
    assert!(
        agent
            .history
            .iter()
            .any(|message| message.content.to_string().contains("tool-204"))
    );
    assert_no_unresolved_tool_uses(&agent.history);
}

#[test]
fn strips_continue_inspection_control_tag() {
    let (cleaned, requested) =
        strip_continue_inspection_control("Need one more inspection pass.\n<continue_inspection/>");
    assert!(requested);
    assert_eq!(cleaned, "Need one more inspection pass.\n");

    let (cleaned, requested) = strip_continue_inspection_control("Final answer");
    assert!(!requested);
    assert_eq!(cleaned, "Final answer");
}

#[test]
fn parses_structured_plan_block() {
    let text = "<proposed_plan>\n- [in_progress] Inspect core agent loop\n- Review TUI rendering path\n1. Confirm current constraints\n</proposed_plan>\nFocus on agent.rs and tui/runtime.rs first.";
    let parsed = parse_plan_block(text).expect("plan block should parse");
    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Inspect core agent loop".to_string(),
                status: PlanStepStatus::InProgress,
            },
            PlanStep {
                step: "Review TUI rendering path".to_string(),
                status: PlanStepStatus::Pending,
            },
            PlanStep {
                step: "Confirm current constraints".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    assert_eq!(
        parsed.1.as_deref(),
        Some("Focus on agent.rs and tui/runtime.rs first.")
    );
}

#[test]
fn parses_structured_proposed_plan_fields_without_mixing_validation_into_steps() {
    let text = "<proposed_plan>\nsummary: Tighten plan exit handling.\nsteps:\n- [pending] Add structured plan prompt guidance\n- [pending] Parse only step entries from the steps section\nvalidation:\n- cargo test exit_plan_mode -- --nocapture\n- cargo check\n</proposed_plan>";
    let parsed = parse_plan_block(text).expect("plan block should parse");
    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Add structured plan prompt guidance".to_string(),
                status: PlanStepStatus::Pending,
            },
            PlanStep {
                step: "Parse only step entries from the steps section".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    let explanation = parsed.1.expect("structured metadata should be preserved");
    assert!(explanation.contains("summary: Tighten plan exit handling."));
    assert!(explanation.contains("validation:"));
    assert!(explanation.contains("cargo test exit_plan_mode -- --nocapture"));
    assert!(!parsed.0.iter().any(|step| step.step.contains("cargo test")));
}

#[test]
fn parses_structured_proposed_plan_headers_case_insensitively() {
    let text = "<proposed_plan>\nSummary: Tighten plan exit handling.\nSteps:\n- [pending] Add structured plan prompt guidance\nValidation:\n- cargo test exit_plan_mode -- --nocapture\n</proposed_plan>";
    let parsed = parse_plan_block(text).expect("plan block should parse");

    assert_eq!(
        parsed.0,
        vec![PlanStep {
            step: "Add structured plan prompt guidance".to_string(),
            status: PlanStepStatus::Pending,
        }]
    );
    let explanation = parsed.1.expect("structured metadata should be preserved");
    assert!(explanation.contains("summary: Tighten plan exit handling."));
    assert!(explanation.contains("validation:"));
    assert!(explanation.contains("cargo test exit_plan_mode -- --nocapture"));
}

#[test]
fn rejects_structured_proposed_plan_without_executable_steps() {
    let text = "<proposed_plan>\nsummary: Missing executable steps.\nsteps:\nvalidation:\n- cargo check\n</proposed_plan>";

    assert!(parse_plan_block(text).is_none());
}

#[test]
fn parses_structured_plan_from_exit_tool_input() {
    let parsed = parse_exit_plan_tool_input(&json!({
        "proposed_plan": {
            "summary": "Use tool arguments as the primary plan transport.",
            "steps": [
                { "step": "Add a proposed_plan schema to exit_plan_mode", "status": "completed" },
                { "step": "Capture structured tool input in plan mode", "status": "pending" }
            ],
            "validation": [
                "cargo test exit_plan_mode -- --nocapture"
            ]
        }
    }))
    .expect("structured tool input should parse");

    assert_eq!(
        parsed.0,
        vec![
            PlanStep {
                step: "Add a proposed_plan schema to exit_plan_mode".to_string(),
                status: PlanStepStatus::Completed,
            },
            PlanStep {
                step: "Capture structured tool input in plan mode".to_string(),
                status: PlanStepStatus::Pending,
            },
        ]
    );
    assert_eq!(
        parsed.1.as_deref(),
        Some(
            "summary: Use tool arguments as the primary plan transport.\nvalidation:\n- cargo test exit_plan_mode -- --nocapture"
        )
    );
}

#[test]
fn detects_unclosed_proposed_plan_block() {
    assert!(has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] Update planning state"
    ));
    assert!(!has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] Update planning state\n</proposed_plan>"
    ));
    assert!(has_unclosed_proposed_plan_block(
        "<proposed_plan>\n- [pending] First\n</proposed_plan>\n<proposed_plan>\n- [pending] Second"
    ));
    assert!(!has_unclosed_proposed_plan_block(
        "Ordinary planning answer without a structured plan."
    ));
}

#[test]
fn parses_request_user_input_block() {
    let text = "<request_user_input>\nquestion: Which path should we take first?\noption: Minimal | Keep the diff small and local.\noption: Broad | Reshape the module boundaries now.\n</request_user_input>\nNeed direction before editing.";
    let parsed = parse_request_user_input_block(text).expect("question block should parse");
    assert_eq!(
        parsed,
        PendingUserInput {
            question: "Which path should we take first?".to_string(),
            options: vec![
                (
                    "Minimal".to_string(),
                    "Keep the diff small and local.".to_string(),
                ),
                (
                    "Broad".to_string(),
                    "Reshape the module boundaries now.".to_string(),
                ),
            ],
            note: Some("Need direction before editing.".to_string()),
        }
    );
}

fn new_planning_agent() -> Agent {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent
}

#[test]
fn shallow_initial_plan_continues_even_after_plan_update() {
    let mut agent = new_planning_agent();
    agent.current_plan = vec![PlanStep {
        step: "Inspect code".to_string(),
        status: PlanStepStatus::Pending,
    }];

    assert!(agent.should_continue_plan_without_tools(true, false, true, false, 0,));
}

#[test]
fn missing_minimum_review_evidence_continues_without_plan_update() {
    let mut agent = new_planning_agent();
    agent.inspection_progress.source_reads = 1;

    assert!(agent.should_continue_plan_without_tools(false, false, true, false, 1,));
}

#[test]
fn reasoning_only_plan_turn_signals_continuation() {
    // should_continue_plan_without_tools always returns true for
    // reasoning-only turns. The consecutive-turn cap is enforced in
    // run_agent_loop_with_limit.
    let agent = new_planning_agent();

    assert!(agent.should_continue_plan_without_tools(false, false, false, true, 0));
    assert!(agent.should_continue_plan_without_tools(false, false, false, true, 1));
}

#[test]
fn execute_mode_continuation_requires_structured_inspection_marker() {
    let mut agent = new_planning_agent();
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.inspection_progress.source_reads = 1;

    // Without continue_inspection or reasoning, bare text doesn't continue.
    assert!(!agent.should_continue_execute_without_tools(false, true, false));
    // With continue_inspection, text continues.
    assert!(agent.should_continue_execute_without_tools(true, true, false));
    // Reasoning-only turn always continues at this level (cap in agent loop).
    assert!(agent.should_continue_execute_without_tools(false, false, true));
    // Same — reasoning-only continuation is turn-agnostic here.
    assert!(agent.should_continue_execute_without_tools(false, false, true));

    agent.inspection_progress.source_reads = 2;
    assert!(agent.should_continue_execute_without_tools(true, true, false));
}

#[test]
fn advances_plan_steps_during_execute_mode() {
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(MemoryHandle::new("data/memory")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.current_plan = vec![
        PlanStep {
            step: "Inspect code".to_string(),
            status: PlanStepStatus::Pending,
        },
        PlanStep {
            step: "Apply changes".to_string(),
            status: PlanStepStatus::Pending,
        },
    ];

    agent.ensure_active_plan_step();
    assert_eq!(agent.current_plan[0].status, PlanStepStatus::InProgress);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::Pending);

    agent.advance_plan_step();
    assert_eq!(agent.current_plan[0].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::InProgress);
}

#[test]
fn completes_only_active_plan_step_on_finish() {
    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(SequencedBackend::new(Vec::new())),
        Arc::new(MemoryHandle::new("data/memory")),
        Arc::new(SessionManager::new().expect("session manager")),
        Arc::new(WorkspaceMemory::new().expect("workspace memory")),
    );
    agent.set_execution_mode(AgentExecutionMode::Execute);
    agent.current_plan = vec![
        PlanStep {
            step: "Inspect code".to_string(),
            status: PlanStepStatus::Completed,
        },
        PlanStep {
            step: "Apply changes".to_string(),
            status: PlanStepStatus::InProgress,
        },
        PlanStep {
            step: "Summarize".to_string(),
            status: PlanStepStatus::Pending,
        },
    ];

    agent.complete_active_plan_step();

    assert_eq!(agent.current_plan[0].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[1].status, PlanStepStatus::Completed);
    assert_eq!(agent.current_plan[2].status, PlanStepStatus::Pending);
}

fn assert_no_unresolved_tool_uses(history: &[crate::agent::Message]) {
    let mut pending = Vec::new();
    for message in history {
        if let Some(items) = message.content.as_array() {
            for item in items {
                match item.get("type").and_then(serde_json::Value::as_str) {
                    Some("tool_use") if message.role == "assistant" => {
                        if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                            pending.push(id.to_string());
                        }
                    }
                    Some("tool_result") if message.role == "user" => {
                        if let Some(id) =
                            item.get("tool_use_id").and_then(serde_json::Value::as_str)
                            && let Some(pos) =
                                pending.iter().position(|pending_id| pending_id == id)
                        {
                            pending.remove(pos);
                        }
                    }
                    _ => {}
                }
            }
        }
    }
    assert!(pending.is_empty(), "unresolved tool uses: {pending:?}");
}
