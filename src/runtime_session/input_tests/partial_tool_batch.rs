use std::sync::atomic::{AtomicUsize, Ordering};

use super::*;
use crate::session::SessionManager;
use crate::{ShellApprovalDecision, ToolEvent};

struct CompletedTool(Arc<AtomicUsize>);

#[async_trait]
impl Tool for CompletedTool {
    fn name(&self) -> &str {
        "completed_tool"
    }

    fn description(&self) -> &str {
        "Return evidence before another call requests approval"
    }

    fn input_schema(&self) -> Value {
        json!({"type": "object"})
    }

    async fn call(&self, _input: Value) -> std::result::Result<Value, ToolError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(json!({"evidence": "completed before approval"}))
    }
}

fn result_blocks(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .filter_map(|message| message.content.as_array())
        .flatten()
        .filter(|block| block["type"] == "tool_result")
        .cloned()
        .collect()
}

#[tokio::test]
async fn approval_pause_retains_completed_results_in_readback_checkpoint_and_resume() {
    for decision in [
        ShellApprovalDecision::Once,
        ShellApprovalDecision::Suggestion,
    ] {
        let root = tempfile::tempdir().expect("workspace");
        let backend = Arc::new(ScriptedBackend::new(vec![
            response(
                vec![
                    ContentBlock::ToolUse {
                        id: "completed-1".into(),
                        name: "completed_tool".into(),
                        input: json!({}),
                    },
                    ContentBlock::ToolUse {
                        id: "pending-2".into(),
                        name: "bash".into(),
                        input: json!({
                            "command": "touch fixture-only",
                            "sandbox_permissions": "require_escalated",
                            "justification": "Partial batch fixture"
                        }),
                    },
                    ContentBlock::ToolUse {
                        id: "later-3".into(),
                        name: "completed_tool".into(),
                        input: json!({}),
                    },
                ],
                10,
            ),
            text_response("done", 20),
        ]));
        let completed_calls = Arc::new(AtomicUsize::new(0));
        let shell = Arc::new(RecordedShell::default());
        let mut tools = ToolManager::new();
        tools.register(Box::new(CompletedTool(completed_calls.clone())));
        tools.register(Box::new(RecordingTool(shell.clone())));
        let state_root = root.path().join("state");
        let session = RuntimeSessionBuilder::for_host(
            RaraConfig::default(),
            root.path(),
            backend.clone(),
            tools,
        )
        .with_state_root(&state_root)
        .with_transcript_persistence()
        .build()
        .await
        .expect("host session");
        session
            .set_full_access_mode(false)
            .await
            .expect("approval policy");
        let first = finish(
            session
                .submit_input(RuntimeInput::Prompt("collect evidence then ask".into()))
                .await
                .expect("first turn"),
        )
        .await;
        let pending = session.snapshot().pending_input.expect("pending shell");
        assert_eq!(pending.turn_id, first.turn_id);
        assert!(
            matches!(pending.kind, RuntimePendingInputKind::Shell { approval_id, .. } if approval_id == "pending-2")
        );
        assert_eq!(backend.request_count(), 1, "pause must not call the model");
        assert_eq!(completed_calls.load(Ordering::SeqCst), 1);
        assert!(shell.calls.lock().expect("shell calls").is_empty());

        let completed = result_blocks(&first.transcript);
        assert_eq!(completed.len(), 1, "keep only the completed call's result");
        assert_eq!(completed[0]["tool_use_id"], "completed-1");
        let evidence = completed[0]["content"].as_str().expect("result text");
        assert!(evidence.contains("completed before approval"));
        assert!(events(&session).await.iter().any(|event| matches!(
            &event.event,
            RuntimeEvent::Tool(ToolEvent::Result { call_id: Some(call_id), content, is_error: false, .. })
                if call_id == "completed-1" && content == evidence
        )));
        assert_ne!(completed[0]["is_error"], true);
        assert_eq!(
            session.transcript().await.expect("paused readback"),
            first.transcript
        );
        let storage = SessionManager::new_for_rara_dir(
            rara_config::workspace_data_dir_for_home(root.path(), &state_root)
                .expect("workspace state"),
        )
        .expect("session storage");
        assert_eq!(
            result_blocks(
                &storage
                    .load_thread_history(session.id().as_str())
                    .expect("checkpoint")
            ),
            completed
        );

        let reply = finish(
            session
                .submit_input(RuntimeInput::Answer {
                    waiting_turn: first.turn_id,
                    answer: RuntimeInputAnswer::Shell { decision },
                })
                .await
                .expect("approval answer"),
        )
        .await;
        assert_eq!(backend.request_count(), 2);
        assert_eq!(
            completed_calls.load(Ordering::SeqCst),
            1,
            "no replay or later call"
        );
        let expected_shell_calls = usize::from(decision == ShellApprovalDecision::Once);
        assert_eq!(
            shell.calls.lock().expect("shell calls").len(),
            expected_shell_calls
        );
        let requests = backend.requests.lock().expect("requests").clone();
        for messages in [&requests[1], &reply.transcript] {
            let results = result_blocks(messages);
            assert_eq!(
                results
                    .iter()
                    .filter(|block| block["tool_use_id"] == "completed-1")
                    .cloned()
                    .collect::<Vec<_>>(),
                completed,
                "the next request must retain the original evidence exactly once"
            );
            let pending_results = results
                .iter()
                .filter(|block| block["tool_use_id"] == "pending-2")
                .collect::<Vec<_>>();
            assert_eq!(pending_results.len(), 1);
            assert_eq!(
                pending_results[0]["is_error"] == true,
                decision == ShellApprovalDecision::Suggestion
            );
        }
        session.shutdown().await.expect("shutdown");
    }
}
