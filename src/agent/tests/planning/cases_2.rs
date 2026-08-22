#[tokio::test]
async fn suggestion_mode_auto_allows_read_only_bash_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-readonly-bash".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git status --short" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "inspect git state".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should auto-allow read-only bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn suggestion_mode_keeps_write_bash_commands_pending_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-write-bash".to_string(),
            name: "bash".to_string(),
            input: json!({ "command": "git push origin main" }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on write bash approval");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn denied_bash_approval_is_recorded_as_tool_failure_for_next_turn() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-denied-bash".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will retry when the user confirms the denial was accidental.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on write bash approval");
    assert!(agent.pending_approval.is_some());

    agent
        .answer_pending_approval_with_events(
            BashApprovalDecision::Suggestion,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("denied approval should continue the agent turn");

    assert!(agent.pending_approval.is_none());
    let observed_messages = backend.observed_messages();
    assert_eq!(observed_messages.len(), 2);
    let resumed_history = &observed_messages[1];
    let tool_call_index = resumed_history
        .iter()
        .position(|message| {
            message.role == "assistant"
                && message.content.to_string().contains("tool-denied-bash")
                && message.content.to_string().contains("git push origin main")
        })
        .expect("assistant tool call should remain in history");
    let denial_result_index = resumed_history
        .iter()
        .position(|message| {
            let content = message.content.to_string();
            message.role == "user"
                && content.contains("\"tool_use_id\":\"tool-denied-bash\"")
                && content.contains("\"is_error\":true")
                && content.contains("rejected by user")
                && content.contains("The command was not run")
        })
        .expect("denied approval should be recorded as an errored tool result");
    let continuation_index = resumed_history
        .iter()
        .position(|message| {
            message.role == "user"
                && message.content.to_string().contains("<agent_runtime>")
                && message
                    .content
                    .to_string()
                    .contains("tool_results_available")
        })
        .expect("runtime continuation should follow the denied tool result");
    assert!(
        tool_call_index < denial_result_index,
        "tool result must follow its assistant tool call"
    );
    assert!(
        denial_result_index < continuation_index,
        "runtime continuation must follow the denied tool result"
    );
}

#[tokio::test]
async fn suggestion_mode_uses_escalated_sandbox_justification_for_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-escalated-bash".to_string(),
            name: "bash".to_string(),
            input: json!({
                "program": "cargo",
                "args": ["check"],
                "sandbox_permissions": "require_escalated",
                "justification": "Do you want to run cargo check outside the sandbox?",
                "prefix_rule": ["cargo", "check"]
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on escalated bash approval");

    assert!(agent.pending_approval.is_some());
    assert!(
        agent.pending_user_input.is_none(),
        "bash approval should stay on the structured approval path"
    );
    assert_eq!(
        agent
            .pending_approval
            .as_ref()
            .and_then(|approval| approval.request.approval_prefix()),
        Some("cargo check".to_string())
    );
}

#[tokio::test]
async fn always_mode_still_requires_approval_for_escalated_sandbox_request() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-escalated-bash".to_string(),
            name: "bash".to_string(),
            input: json!({
                "program": "cargo",
                "args": ["check"],
                "sandbox_permissions": "require_escalated"
            }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Always;

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on escalated bash approval");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn full_access_mode_auto_allows_escalated_sandbox_request() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-escalated-bash".to_string(),
                name: "bash".to_string(),
                input: json!({
                    "program": "cargo",
                    "args": ["check"],
                    "sandbox_permissions": "require_escalated"
                }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Always;
    agent.set_full_access_mode(true);

    agent
        .query_with_mode(
            "run check with full access".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("full access should auto-allow escalated bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn full_access_mode_bypasses_auto_permission_classifier_denials() {
    let backend = Arc::new(
        SequencedBackend::new(vec![
            LlmResponse {
                content: vec![ContentBlock::ToolUse {
                    id: "tool-system-path".to_string(),
                    name: "bash".to_string(),
                    input: json!({
                        "command": "ln -sf /app/tool /usr/local/bin/tool",
                        "sandbox_permissions": "require_escalated"
                    }),
                }],
                stop_reason: Some("tool_use".to_string()),
                usage: Some(TokenUsage::default()),
            },
            LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "done".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage::default()),
            },
        ])
        .with_classifier_response(
            r#"{"decision":"deny","reason":"outside workspace","matched_rule":"workspace"}"#,
        ),
    );
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.set_full_access_mode(true);

    agent
        .query_with_mode(
            "install the command in the externally isolated container".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("full access should execute the command");

    assert_eq!(backend.classifier_call_count(), 0);
    assert!(backend.observed_messages()[1].iter().any(|message| {
        message.role == "user"
            && message
                .content
                .to_string()
                .contains("finished with exit code 0")
    }));
}

#[tokio::test]
async fn approved_prefix_auto_allows_matching_escalated_request() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-escalated-bash".to_string(),
                name: "bash".to_string(),
                input: json!({
                    "program": "cargo",
                    "args": ["check"],
                    "sandbox_permissions": "require_escalated",
                    "prefix_rule": ["cargo", "check"]
                }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;
    agent.approved_bash_prefixes.push("cargo check".to_string());

    agent
        .query_with_mode(
            "run check outside sandbox".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should auto-allow escalated bash by approved prefix");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn plan_mode_allows_read_only_bash_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-readonly-bash-plan".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git status --short" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "Read-only inspection complete.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
            "inspect git state".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should allow read-only bash in plan mode");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
}

#[tokio::test]
async fn plan_mode_rejects_mutating_bash_commands_without_approval() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-write-bash-plan".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "I will return a plan instead.".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
            "push changes".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should reject mutating bash and continue");

    assert_eq!(agent.execution_mode, AgentExecutionMode::Plan);
    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 2);
    assert!(agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("bash is read-only in plan mode")
    }));
}

#[tokio::test]
async fn approved_bash_prefix_auto_allows_later_matching_commands() {
    let backend = Arc::new(SequencedBackend::new(vec![
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-first-push".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin main" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "first push done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::ToolUse {
                id: "tool-second-push".to_string(),
                name: "bash".to_string(),
                input: json!({ "command": "git push origin feature" }),
            }],
            stop_reason: Some("tool_use".to_string()),
            usage: Some(TokenUsage::default()),
        },
        LlmResponse {
            content: vec![ContentBlock::Text {
                text: "second push done".to_string(),
            }],
            stop_reason: Some("end_turn".to_string()),
            usage: Some(TokenUsage::default()),
        },
    ]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;

    agent
        .query_with_mode(
            "push once".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("first query should pause on approval");
    assert!(agent.pending_approval.is_some());

    agent
        .answer_pending_approval_with_events(
            BashApprovalDecision::Prefix,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("prefix approval should execute pending command");
    assert_eq!(agent.approved_bash_prefixes, vec!["git push".to_string()]);

    agent
        .query_with_mode(
            "push matching prefix".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("matching prefix should auto-allow bash");

    assert!(agent.pending_approval.is_none());
    assert_eq!(backend.observed_messages().len(), 4);
}

#[tokio::test]
async fn approved_bash_prefix_does_not_auto_allow_unapproved_shell_segments() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::ToolUse {
            id: "tool-chained-push-rm".to_string(),
            name: "bash".to_string(),
            input: json!({ "command": "git push origin main && rm -rf target" }),
        }],
        stop_reason: Some("tool_use".to_string()),
        usage: Some(TokenUsage::default()),
    }]));
    let mut tool_manager = ToolManager::new();
    tool_manager.register(Box::new(StubBashTool));
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
    agent.bash_approval_mode = crate::agent::BashApprovalMode::Suggestion;
    agent.approved_bash_prefixes.push("git push".to_string());

    agent
        .query_with_mode(
            "push and clean".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should pause on the unapproved chained segment");

    assert!(agent.pending_approval.is_some());
    assert_eq!(backend.observed_messages().len(), 1);
}

#[tokio::test]
async fn checkpoints_user_message_before_first_model_turn() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let session_id = "checkpoint-before-model".to_string();
    let backend = Arc::new(CheckpointObserverBackend {
        session_manager: session_manager.clone(),
        session_id: session_id.clone(),
    });
    let mut agent = Agent::new(
        ToolManager::new(),
        backend,
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        session_manager,
        workspace,
    );
    agent.session_id = session_id;
    agent.tool_result_store =
        ToolResultStore::new(rara_dir.join("tool-results")).expect("tool result store");

    agent
        .query_with_mode(
            "checkpoint me".to_string(),
            super::super::AgentOutputMode::Silent,
        )
        .await
        .expect("query should succeed");
}

#[tokio::test]
async fn resumes_after_plan_approval_via_structured_continuation() {
    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "Implemented the first plan step.".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new("data/memory")),
        session_manager,
        workspace,
    );
    agent.tool_result_store =
        ToolResultStore::new(rara_dir.join("tool-results")).expect("tool result store");
    agent.set_execution_mode(AgentExecutionMode::Plan);
    agent.current_plan = vec![PlanStep {
        step: "Modify workspace instruction discovery".to_string(),
        status: PlanStepStatus::Pending,
    }];

    agent
        .resume_after_plan_approval_with_events(
            false,
            super::super::AgentOutputMode::Silent,
            |_| {},
        )
        .await
        .expect("resume should succeed");

    let observed = backend.observed_messages();
    assert_eq!(observed.len(), 1);
    let runtime_texts = observed[0]
        .iter()
        .filter_map(|message| message.content.as_array())
        .flat_map(|blocks| blocks.iter())
        .filter_map(|block| block.get("text").and_then(serde_json::Value::as_str))
        .collect::<Vec<_>>();
    assert!(
        runtime_texts
            .iter()
            .any(|text| text.contains("\"phase\": \"plan_approved\""))
    );
    assert!(
        runtime_texts
            .iter()
            .any(|text| text.contains("\"mode\": \"execute\""))
    );
    assert!(!runtime_texts.iter().any(|text| {
        text.contains("Implement the approved plan using the current repository state")
    }));
}

#[tokio::test]
async fn does_not_append_continuation_without_tools() {
    let backend = Arc::new(SequencedBackend::new(vec![LlmResponse {
        content: vec![ContentBlock::Text {
            text: "final".to_string(),
        }],
        stop_reason: Some("end_turn".to_string()),
        usage: Some(TokenUsage::default()),
    }]));

    let (_temp, session_manager, workspace, rara_dir) = test_runtime_storage();
    let mut agent = Agent::new(
        ToolManager::new(),
        backend.clone(),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").to_string_lossy(),
        )),
        session_manager,
        workspace,
    );

    agent
        .query_with_mode("hello".to_string(), super::super::AgentOutputMode::Silent)
        .await
        .expect("query should succeed");

    assert_eq!(backend.observed_messages().len(), 1);
    assert!(!agent.history.iter().any(|message| {
        message
            .content
            .to_string()
            .contains("\"phase\": \"tool_results_available\"")
    }));
}
