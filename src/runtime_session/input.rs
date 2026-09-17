use serde::{Deserialize, Serialize};

use super::{RuntimeSessionError, RuntimeTurnId};
use crate::agent::{Agent, AgentEvent, AgentOutputMode};
use crate::runtime_control::{InputControlRequest, PlanApprovalDecision, ShellApprovalDecision};
use crate::tools::bash::BashCommandInput;

/// Strict session input. Replies name the turn that created their pending interaction.
#[derive(Clone, Debug)]
pub enum RuntimeInput {
    Prompt(String),
    FollowUp(String),
    Answer {
        waiting_turn: RuntimeTurnId,
        answer: RuntimeInputAnswer,
    },
}

/// A native response whose kind must match the currently pending interaction.
#[derive(Clone, Debug)]
pub enum RuntimeInputAnswer {
    User {
        answer: String,
    },
    Plan {
        decision: PlanApprovalDecision,
        feedback: Option<String>,
    },
    Shell {
        decision: ShellApprovalDecision,
    },
}

/// Session-owned interaction state, separate from active provider execution.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimePendingInput {
    pub turn_id: RuntimeTurnId,
    pub kind: RuntimePendingInputKind,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", content = "payload", rename_all = "snake_case")]
pub enum RuntimePendingInputKind {
    User {
        question: String,
        options: Vec<(String, String)>,
        note: Option<String>,
    },
    Plan {
        approval_id: String,
        plan: String,
    },
    Shell {
        approval_id: String,
        request: Box<BashCommandInput>,
    },
}

pub(super) enum TurnInput {
    LegacyPrompt(String),
    Controlled(RuntimeInput),
}

impl RuntimePendingInput {
    pub(super) fn from_agent(turn_id: RuntimeTurnId, agent: &Agent) -> Option<Self> {
        // A plain text reply must never bypass a native tool or plan approval.
        let kind = if let Some(pending) = &agent.pending_approval {
            RuntimePendingInputKind::Shell {
                approval_id: pending.tool_use_id.clone(),
                request: Box::new(pending.request.clone()),
            }
        } else if let Some(approval_id) = agent.pending_plan_exit_tool_id() {
            RuntimePendingInputKind::Plan {
                approval_id: approval_id.to_owned(),
                plan: agent.current_plan_markdown(),
            }
        } else if let Some(pending) = &agent.pending_user_input {
            RuntimePendingInputKind::User {
                question: pending.question.clone(),
                options: pending.options.clone(),
                note: pending.note.clone(),
            }
        } else {
            return None;
        };
        Some(Self { turn_id, kind })
    }
}

impl TurnInput {
    pub(super) fn is_answer(&self) -> bool {
        matches!(self, Self::Controlled(RuntimeInput::Answer { .. }))
    }

    pub(super) fn validate(
        &self,
        pending: Option<&RuntimePendingInput>,
    ) -> Result<(), RuntimeSessionError> {
        match self {
            Self::LegacyPrompt(_) => Ok(()),
            Self::Controlled(RuntimeInput::Prompt(_) | RuntimeInput::FollowUp(_)) => {
                if let Some(pending) = pending {
                    Err(RuntimeSessionError::AwaitingInput {
                        waiting_turn: pending.turn_id.clone(),
                    })
                } else {
                    Ok(())
                }
            }
            Self::Controlled(RuntimeInput::Answer {
                waiting_turn,
                answer,
            }) => {
                let pending = pending.ok_or(RuntimeSessionError::NoPendingInput)?;
                if waiting_turn != &pending.turn_id {
                    return Err(RuntimeSessionError::StaleInput {
                        expected: waiting_turn.clone(),
                        waiting: pending.turn_id.clone(),
                    });
                }
                let matches = match answer {
                    RuntimeInputAnswer::User { .. } => {
                        matches!(pending.kind, RuntimePendingInputKind::User { .. })
                    }
                    RuntimeInputAnswer::Plan { .. } => {
                        matches!(pending.kind, RuntimePendingInputKind::Plan { .. })
                    }
                    RuntimeInputAnswer::Shell { .. } => {
                        matches!(pending.kind, RuntimePendingInputKind::Shell { .. })
                    }
                };
                if matches {
                    Ok(())
                } else {
                    Err(RuntimeSessionError::InputKindMismatch)
                }
            }
        }
    }

    pub(super) async fn execute<F>(
        self,
        agent: &mut Agent,
        output_mode: AgentOutputMode,
        report: F,
    ) -> anyhow::Result<()>
    where
        F: FnMut(AgentEvent) + Send,
    {
        match self {
            Self::LegacyPrompt(prompt) => {
                agent
                    .query_with_mode_and_events(prompt, output_mode, report)
                    .await
            }
            Self::Controlled(input) => {
                let request = match input {
                    RuntimeInput::Prompt(prompt) => {
                        InputControlRequest::SubmitUserPrompt { prompt }
                    }
                    RuntimeInput::FollowUp(prompt) => {
                        InputControlRequest::SubmitFollowUp { prompt }
                    }
                    RuntimeInput::Answer { answer, .. } => match answer {
                        RuntimeInputAnswer::User { answer } => {
                            InputControlRequest::AnswerPendingInput { answer }
                        }
                        RuntimeInputAnswer::Plan { decision, feedback } => {
                            InputControlRequest::AnswerPlanApproval { decision, feedback }
                        }
                        RuntimeInputAnswer::Shell { decision } => {
                            InputControlRequest::AnswerShellApproval { decision }
                        }
                    },
                };
                agent.handle_input_control(&request, report).await
            }
        }
    }
}
