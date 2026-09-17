use std::sync::atomic::Ordering;

use anyhow::{Result, anyhow};

use crate::agent::{Agent, AgentEvent, AgentOutputMode, BashApprovalDecision};
use crate::runtime_control::{InputControlRequest, PlanApprovalDecision, SessionControlRequest};

impl Agent {
    pub(crate) fn discard_pending_interactions(&mut self) {
        self.pending_user_input = None;
        self.pending_approval = None;
        self.pending_plan_exit_tool_id = None;
    }

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
        mut report: F,
    ) -> Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        let _lease = match request {
            InputControlRequest::AnswerPlanApproval { decision, .. } => {
                if !self.has_pending_plan_exit_approval() {
                    return Err(anyhow!("no pending plan approval"));
                }
                let lease = self.begin_inference_turn();
                if !matches!(decision, PlanApprovalDecision::Reject) {
                    self.refresh_protocol_prompt_sources_for_query().await;
                    self.refresh_protocol_skill_sources_for_query().await?;
                }
                Some(lease)
            }
            InputControlRequest::AnswerShellApproval { .. } => {
                if self.pending_approval.is_none() {
                    return Err(anyhow!("no pending shell approval"));
                }
                let lease = self.begin_inference_turn();
                self.refresh_protocol_prompt_sources_for_query().await;
                self.refresh_protocol_skill_sources_for_query().await?;
                Some(lease)
            }
            InputControlRequest::AnswerPendingInput { .. } => {
                if self.pending_user_input.is_none() {
                    return Err(anyhow!("no pending user input"));
                }
                None
            }
            InputControlRequest::SubmitUserPrompt { .. }
            | InputControlRequest::SubmitFollowUp { .. } => None,
        };
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
                    let approval_id = self
                        .pending_plan_exit_tool_id()
                        .ok_or_else(|| anyhow!("no pending plan approval"))?
                        .to_owned();
                    self.reject_pending_plan_approval(feedback.as_deref())?;
                    report(AgentEvent::ApprovalAnswered {
                        approval_id,
                        approved: false,
                    });
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
