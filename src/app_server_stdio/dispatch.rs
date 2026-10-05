use rara_app_server::runtime_control::{
    InputControlRequest, RuntimeControlEnvelope, RuntimeControlRequest, RuntimeControllerKind,
    RuntimeProvenance, SessionControlRequest, SkillSourceControlRequest,
};
use rara_app_server::stdio_protocol::{RejectionCode, RequestResult};

use super::io::Failure;
use crate::runtime_session::{
    RuntimeInput, RuntimeInputAnswer, RuntimeSession, RuntimeSessionError, RuntimeTurnId,
};

pub(super) enum DispatchError {
    Rejected(RejectionCode),
    Fatal(Failure),
}

impl From<RuntimeSessionError> for DispatchError {
    fn from(error: RuntimeSessionError) -> Self {
        use RuntimeSessionError as E;
        let code = match error {
            E::Busy { .. } | E::AwaitingInput { .. } | E::StopInProgress { .. } => {
                RejectionCode::Busy
            }
            E::NotRunning | E::NoPendingInput => RejectionCode::NotRunning,
            E::StaleInput { .. }
            | E::InputKindMismatch
            | E::StaleTurn { .. }
            | E::InvalidSource => RejectionCode::InvalidRequest,
            E::UnsupportedSource => RejectionCode::Unsupported,
            E::SourceCapacity | E::Overloaded => RejectionCode::Overloaded,
            E::Closed => RejectionCode::Closed,
            E::AlreadyExists(_) => RejectionCode::RequestConflict,
            E::ResyncRequired { .. } | E::EventLagged(_) => return Self::Fatal(Failure::ReplayGap),
            // An owner failure after dispatch is not evidence of non-admission.
            E::SourceUnavailable
            | E::ShutdownFailed
            | E::ActorStopped
            | E::Cancelled { .. }
            | E::Interrupted { .. }
            | E::Execution { .. } => return Self::Fatal(Failure::Runtime),
        };
        Self::Rejected(code)
    }
}

pub(super) async fn control(
    session: &RuntimeSession,
    envelope: &RuntimeControlEnvelope,
    expected_turn: Option<&str>,
) -> Result<RequestResult, DispatchError> {
    let target = || {
        expected_turn
            .map(RuntimeTurnId::new)
            .ok_or(DispatchError::Rejected(RejectionCode::InvalidRequest))
    };
    let provenance = RuntimeProvenance::protocol(
        RuntimeControllerKind::AppServer,
        "stdio-jsonl",
        Some(session.id().to_string()),
        envelope.provenance.source_id.clone(),
    );
    let mut turn_id = None;
    match &envelope.request {
        RuntimeControlRequest::Session(request) => match request {
            SessionControlRequest::CreateSession | SessionControlRequest::ResumeSession { .. } => {
                return Err(DispatchError::Rejected(RejectionCode::Unsupported));
            }
            SessionControlRequest::CancelCurrentTurn => {
                turn_id = Some(session.cancel_turn(&target()?).await?.to_string());
            }
            SessionControlRequest::InterruptCurrentTurn => {
                turn_id = Some(session.interrupt_turn(&target()?).await?.to_string());
            }
            SessionControlRequest::QueryRuntimeState => session.query_runtime_state().await?,
        },
        RuntimeControlRequest::Input(request) => {
            let input = match request {
                InputControlRequest::SubmitUserPrompt { prompt } => {
                    RuntimeInput::Prompt(prompt.clone())
                }
                InputControlRequest::SubmitFollowUp { prompt } => {
                    RuntimeInput::FollowUp(prompt.clone())
                }
                InputControlRequest::AnswerPendingInput { answer } => RuntimeInput::Answer {
                    waiting_turn: target()?,
                    answer: RuntimeInputAnswer::User {
                        answer: answer.clone(),
                    },
                },
                InputControlRequest::AnswerPlanApproval { decision, feedback } => {
                    RuntimeInput::Answer {
                        waiting_turn: target()?,
                        answer: RuntimeInputAnswer::Plan {
                            decision: *decision,
                            feedback: feedback.clone(),
                        },
                    }
                }
                InputControlRequest::AnswerShellApproval { decision } => RuntimeInput::Answer {
                    waiting_turn: target()?,
                    answer: RuntimeInputAnswer::Shell {
                        decision: *decision,
                    },
                },
            };
            turn_id = Some(session.submit_input(input).await?.id().to_string());
        }
        RuntimeControlRequest::PromptSource(request) => {
            session
                .apply_prompt_source(request.clone(), provenance)
                .await?
        }
        RuntimeControlRequest::SkillSource(request) => match request {
            SkillSourceControlRequest::RegisterRoot { .. } => {
                return Err(DispatchError::Rejected(RejectionCode::Unsupported));
            }
            SkillSourceControlRequest::RegisterSkill { .. }
            | SkillSourceControlRequest::DisableSkill { .. }
            | SkillSourceControlRequest::QuerySkills => {
                session
                    .apply_skill_source(request.clone(), provenance)
                    .await?
            }
        },
        RuntimeControlRequest::Output(_)
        | RuntimeControlRequest::Mcp(_)
        | RuntimeControlRequest::Memory(_)
        | RuntimeControlRequest::Hook(_)
        | RuntimeControlRequest::Approval(_) => {
            return Err(DispatchError::Rejected(RejectionCode::Unsupported));
        }
    }
    Ok(RequestResult::Accepted {
        session_id: Some(session.id().to_string()),
        turn_id,
        last_sequence: Some(session.snapshot().last_sequence),
    })
}

pub(super) fn rejection(code: RejectionCode) -> RequestResult {
    let message = match code {
        RejectionCode::InvalidRequest => "request is invalid for this operation",
        RejectionCode::StaleRuntime => "request targets a different runtime",
        RejectionCode::UnknownSession => "request targets an unknown session",
        RejectionCode::Unsupported => "operation is unsupported",
        RejectionCode::Busy => "session is busy or waiting for a different interaction",
        RejectionCode::NotRunning => "session has no matching active operation",
        RejectionCode::Overloaded => "runtime capacity is exhausted",
        RejectionCode::Closed => "session is closed",
        RejectionCode::RequestConflict => "request identity conflicts with retained content",
        RejectionCode::Internal => "runtime could not apply this operation",
    };
    RequestResult::Rejected {
        code,
        message: message.into(),
    }
}
