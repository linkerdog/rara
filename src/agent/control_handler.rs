use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow};

use crate::agent::{Agent, AgentEvent, AgentExecutionMode, AgentOutputMode, BashApprovalDecision};
use crate::runtime_control::{InputControlRequest, SessionControlRequest, ShellApprovalDecision};

impl Agent {
    /// Handle a session control request.
    pub async fn handle_session_control(&mut self, request: &SessionControlRequest) -> Result<()> {
        match request {
            SessionControlRequest::CancelCurrentTurn => {
                if let Some(token) = self.cancellation_token.as_ref() {
                    token.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            SessionControlRequest::InterruptCurrentTurn => {
                if let Some(token) = self.cancellation_token.as_ref() {
                    token.store(true, Ordering::SeqCst);
                }
                Ok(())
            }
            SessionControlRequest::QueryRuntimeState => {
                // Trigger state refresh.
                Ok(())
            }
            _ => Ok(()),
        }
    }

    /// Handle an input control request.
    pub async fn handle_input_control<F>(
        &mut self,
        request: &InputControlRequest,
        report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        match request {
            InputControlRequest::SubmitUserPrompt { prompt } => {
                self.query_with_mode_and_events(prompt.clone(), AgentOutputMode::Silent, report)
                    .await?;
            }
            InputControlRequest::AnswerPendingInput { answer } => {
                self.consume_pending_user_input(answer);
                self.query_with_mode_and_events(answer.clone(), AgentOutputMode::Silent, report)
                    .await?;
            }
            InputControlRequest::AnswerPlanApproval { approved } => {
                if *approved {
                    self.set_execution_mode(AgentExecutionMode::Execute);
                    self.query_with_mode_and_events(
                        "Plan approved. Proceed with implementation.".to_string(),
                        AgentOutputMode::Silent,
                        report,
                    )
                    .await?;
                } else {
                    self.query_with_mode_and_events(
                        "Plan rejected. Please revise the plan or suggest an alternative."
                            .to_string(),
                        AgentOutputMode::Silent,
                        report,
                    )
                    .await?;
                }
            }
            InputControlRequest::AnswerShellApproval { decision } => {
                let decision = BashApprovalDecision::from(*decision);
                self.answer_pending_approval_with_events(decision, AgentOutputMode::Silent, report)
                    .await?;
            }
            InputControlRequest::SubmitFollowUp { prompt } => {
                // If the agent is idle, we can query. If busy, we need a queue.
                // Headless/ACP doesn't have a queue yet, but we can query directly if idle.
                self.query_with_mode_and_events(prompt.clone(), AgentOutputMode::Silent, report)
                    .await?;
            }
        }
        Ok(())
    }
}
