use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow};

use crate::agent::{Agent, AgentEvent, AgentOutputMode, BashApprovalDecision};
use crate::runtime_control::{InputControlRequest, PlanApprovalDecision, SessionControlRequest};

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
            InputControlRequest::AnswerPlanApproval { decision, feedback } => match decision {
                PlanApprovalDecision::Approve => {
                    self.resume_after_plan_approval_with_events(
                        false,
                        AgentOutputMode::Silent,
                        report,
                    )
                    .await?;
                }
                PlanApprovalDecision::ContinuePlanning => {
                    self.resume_after_plan_approval_with_feedback_events(
                        true,
                        feedback.as_deref(),
                        AgentOutputMode::Silent,
                        report,
                    )
                    .await?;
                }
                PlanApprovalDecision::Reject => {
                    self.reject_pending_plan_approval(feedback.as_deref())?;
                }
            },
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
