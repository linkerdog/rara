use super::*;
use crate::runtime_control::{
    ApprovalEvent, PlanApprovalDecision, PromptSourceControlRequest, PromptSourceLifetime,
    PromptSourceRegistration, RuntimeControllerKind, RuntimeProvenance, ShellApprovalDecision,
    SourceLayer, SourceScope,
};

const SOURCE: &str = "approval-continuation-source";

fn plan_response() -> LlmResponse {
    response(
        vec![
            ContentBlock::Text {
                text: format!(
                    "<proposed_plan>\n- [pending] Make the scoped change\n</proposed_plan>\n{QUESTION}"
                ),
            },
            ContentBlock::ToolUse {
                id: "plan-1".into(),
                name: "exit_plan_mode".into(),
                input: json!({}),
            },
        ],
        10,
    )
}

async fn register_context(session: &RuntimeSession) {
    session
        .apply_prompt_source(
            PromptSourceControlRequest::Register(PromptSourceRegistration {
                source_id: SOURCE.into(),
                scope: SourceScope::Session,
                layer: SourceLayer::User,
                budget_hint_tokens: None,
                lifetime: PromptSourceLifetime::Turns(1),
                content: SOURCE.into(),
            }),
            RuntimeProvenance::protocol(
                RuntimeControllerKind::AppServer,
                "stdio-jsonl",
                Some(session.id().to_string()),
                Some(SOURCE.into()),
            ),
        )
        .await
        .expect("source during wait");
}

#[tokio::test]
async fn generated_plan_without_a_pending_interaction_rejects_protocol_approval() {
    let root = tempfile::tempdir().expect("workspace");
    let backend = Arc::new(ScriptedBackend::new(vec![text_response(
        "<proposed_plan>\n- [pending] Inspect the change\n- [pending] Verify the change\n</proposed_plan>",
        10,
    )]));
    let session = session(
        root.path(),
        backend.clone(),
        Arc::default(),
        AgentExecutionMode::Plan,
    )
    .await;
    let turn = finish(
        session
            .submit_input(RuntimeInput::Prompt("plan".into()))
            .await
            .unwrap(),
    )
    .await;
    assert!(session.snapshot().pending_input.is_none());
    assert!(matches!(
        session
            .submit_input(RuntimeInput::Answer {
                waiting_turn: turn.turn_id,
                answer: RuntimeInputAnswer::Plan {
                    decision: PlanApprovalDecision::Approve,
                    feedback: None
                },
            })
            .await,
        Err(RuntimeSessionError::NoPendingInput)
    ));
    assert_eq!(backend.request_count(), 1);
    session.shutdown().await.expect("shutdown");
}

#[tokio::test]
async fn native_plan_decisions_preserve_identity_sources_and_fresh_accounting() {
    for decision in [
        PlanApprovalDecision::Approve,
        PlanApprovalDecision::ContinuePlanning,
        PlanApprovalDecision::Reject,
    ] {
        let root = tempfile::tempdir().expect("workspace");
        let backend = Arc::new(ScriptedBackend::new(vec![
            plan_response(),
            text_response(
                if decision == PlanApprovalDecision::ContinuePlanning {
                    QUESTION
                } else {
                    "done"
                },
                30,
            ),
        ]));
        let session = session(
            root.path(),
            backend.clone(),
            Arc::default(),
            AgentExecutionMode::Plan,
        )
        .await;
        let first = session
            .submit_input(RuntimeInput::Prompt("plan a change".into()))
            .await
            .expect("plan turn");
        let first_ledger = first.accounting();
        let first = finish(first).await;
        assert!(
            matches!(session.snapshot().pending_input.unwrap().kind, RuntimePendingInputKind::Plan { approval_id, plan } if approval_id == "plan-1" && plan.contains("Make the scoped change"))
        );
        assert!(
            matches!(
                session.submit_input(user_answer(&first.turn_id)).await,
                Err(RuntimeSessionError::InputKindMismatch)
            ),
            "a simultaneous question must not bypass the plan approval"
        );
        register_context(&session).await;
        let answer = RuntimeInput::Answer {
            waiting_turn: first.turn_id.clone(),
            answer: RuntimeInputAnswer::Plan {
                decision,
                feedback: Some("Keep the public API stable.".into()),
            },
        };
        let reply = finish(
            session
                .submit_input(answer.clone())
                .await
                .expect("plan answer"),
        )
        .await;
        assert_eq!(
            first_ledger.snapshot().calls.len(),
            1,
            "answers must not charge the preceding task"
        );
        assert!(events(&session).await.iter().any(|event| matches!(&event.event, RuntimeEvent::Approval(ApprovalEvent::Answered { approval_id, approved }) if approval_id == "plan-1" && *approved == (decision == PlanApprovalDecision::Approve)) && event.turn_id.as_deref() == Some(reply.turn_id.as_str())));
        assert!(
            session.submit_input(answer).await.is_err(),
            "a decision can only be consumed once"
        );
        if decision == PlanApprovalDecision::Reject {
            assert!(
                reply.query_report.model_turns.is_empty(),
                "rejection must not repeat the old usage report"
            );
            assert_eq!(backend.request_count(), 1);
            assert_eq!(session.snapshot().phase, RuntimeSessionPhase::Idle);
            assert!(reply.transcript.iter().any(|message| {
                message
                    .content
                    .to_string()
                    .contains("User rejected the plan")
            }));
            // The one-query source remains eligible until a provider call is needed.
            finish(
                session
                    .submit_input(RuntimeInput::Prompt("new task".into()))
                    .await
                    .expect("fresh prompt"),
            )
            .await;
        } else {
            assert_eq!(reply.query_report.model_turns.len(), 1);
            assert_eq!(
                reply.query_report.model_turns[0]
                    .usage
                    .unwrap()
                    .input_tokens,
                30
            );
            if decision == PlanApprovalDecision::ContinuePlanning {
                assert!(matches!(
                    session.snapshot().pending_input.unwrap().kind,
                    RuntimePendingInputKind::User { .. }
                ));
            } else {
                assert_eq!(session.snapshot().phase, RuntimeSessionPhase::Idle);
            }
        }
        let requests = backend.requests.lock().expect("requests").clone();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[1]
                .iter()
                .any(|message| message.role == "user"
                    && message.content.to_string().contains(SOURCE)),
            "a continuation must receive the newly registered source"
        );
        if decision == PlanApprovalDecision::ContinuePlanning {
            assert!(requests[1].iter().any(|message| {
                message
                    .content
                    .to_string()
                    .contains("Keep the public API stable.")
            }));
        }
        session.shutdown().await.expect("shutdown");
    }
}

#[tokio::test]
async fn native_shell_decisions_execute_once_or_deny_without_running() {
    for decision in [
        ShellApprovalDecision::Once,
        ShellApprovalDecision::Suggestion,
    ] {
        let root = tempfile::tempdir().expect("workspace");
        let command = json!({"command": "touch fixture-only", "sandbox_permissions": "require_escalated", "justification": "Native approval fixture", "prefix_rule": ["touch"]});
        let backend = Arc::new(ScriptedBackend::new(vec![
            response(
                vec![ContentBlock::ToolUse {
                    id: "shell-1".into(),
                    name: "bash".into(),
                    input: command.clone(),
                }],
                10,
            ),
            text_response("done", 30),
        ]));
        let shell = Arc::new(RecordedShell::default());
        let session = session(
            root.path(),
            backend.clone(),
            shell.clone(),
            AgentExecutionMode::Execute,
        )
        .await;
        session
            .set_full_access_mode(false)
            .await
            .expect("approval policy");
        let first = finish(
            session
                .submit_input(RuntimeInput::Prompt("record a command".into()))
                .await
                .expect("shell turn"),
        )
        .await;
        let pending = session.snapshot().pending_input.expect("pending shell");
        assert!(
            matches!(pending.kind, RuntimePendingInputKind::Shell { approval_id, request } if approval_id == "shell-1" && request.command.as_deref() == Some("touch fixture-only") && request.justification.as_deref() == Some("Native approval fixture"))
        );
        assert!(shell.calls.lock().expect("shell calls").is_empty());
        assert!(matches!(
            session
                .submit_input(RuntimeInput::Prompt("yes".into()))
                .await,
            Err(RuntimeSessionError::AwaitingInput { .. })
        ));
        let answer = RuntimeInput::Answer {
            waiting_turn: first.turn_id,
            answer: RuntimeInputAnswer::Shell { decision },
        };
        let reply = finish(
            session
                .submit_input(answer.clone())
                .await
                .expect("shell answer"),
        )
        .await;
        assert_eq!(reply.query_report.model_turns.len(), 1);
        assert_eq!(
            reply.query_report.model_turns[0]
                .usage
                .unwrap()
                .input_tokens,
            30
        );
        assert!(matches!(
            session.submit_input(answer).await,
            Err(RuntimeSessionError::NoPendingInput)
        ));
        let calls = shell.calls.lock().expect("shell calls").clone();
        match decision {
            ShellApprovalDecision::Once => {
                assert_eq!(calls.len(), 1);
                assert_eq!(calls[0].0["command"], command["command"]);
                assert_eq!(calls[0].1, reply.turn_id.as_str());
                assert_eq!(calls[0].2, "shell-1");
            }
            ShellApprovalDecision::Suggestion => {
                assert!(calls.is_empty());
                assert!(
                    backend.requests.lock().expect("requests")[1]
                        .iter()
                        .any(|message| message
                            .content
                            .to_string()
                            .contains("bash command rejected by user"))
                );
            }
            ShellApprovalDecision::Prefix | ShellApprovalDecision::Always => unreachable!(),
        }
        assert_eq!(backend.request_count(), 2);
        assert!(events(&session).await.iter().any(|event| matches!(&event.event, RuntimeEvent::Approval(ApprovalEvent::Answered { approval_id, approved }) if approval_id == "shell-1" && *approved == (decision == ShellApprovalDecision::Once))));
        session.shutdown().await.expect("shutdown");
    }
}
