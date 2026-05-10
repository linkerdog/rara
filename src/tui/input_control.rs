use crate::agent::{Agent, BashApprovalDecision};
use crate::runtime_control::{
    InputEvent, RuntimeEvent, SessionControlRequest, SessionEvent, ShellApprovalDecision,
};
use crate::tui::runtime::{
    request_running_task_cancellation, start_input_control_task, start_pending_approval_task,
    start_plan_approval_resume_task, start_query_task,
};
use crate::tui::state::{
    ActivePendingInteractionKind, InteractionKind, RuntimePhase, TaskKind, TuiApp,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InputControlOutcome {
    Noop,
    Queued,
    Submitted,
    Answered,
    CancelRequested,
    Rejected,
}

pub(crate) fn submit_user_prompt(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    prompt: String,
) -> InputControlOutcome {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        if app.bottom_pane.notice.is_none() {
            app.bottom_pane.notice = Some("Ready.".into());
        }
        return InputControlOutcome::Noop;
    }

    if app.is_busy() {
        let pending_for_tool_boundary = app
            .bottom_pane
            .running_task
            .as_ref()
            .is_some_and(|task| matches!(&task.kind, TaskKind::Query));
        return submit_follow_up(app, prompt, pending_for_tool_boundary);
    }

    if app.active_pending_interaction().is_some() && app.pending_request_input().is_none() {
        return submit_follow_up(app, prompt, false);
    }

    let Some(agent) = agent_slot.take() else {
        app.push_notice("Agent is not ready for input.");
        return InputControlOutcome::Rejected;
    };

    if app.pending_request_input().is_some() {
        answer_pending_input(app, agent_slot, agent, prompt);
        return InputControlOutcome::Answered;
    }

    app.clear_pending_planning_suggestion();
    publish_input_event(app, InputEvent::UserPromptSubmitted);
    start_query_task(app, prompt, agent);
    InputControlOutcome::Submitted
}

pub(crate) fn submit_follow_up(
    app: &mut TuiApp,
    prompt: String,
    release_after_next_tool_boundary: bool,
) -> InputControlOutcome {
    let prompt = prompt.trim().to_string();
    if prompt.is_empty() {
        return InputControlOutcome::Noop;
    }

    let queued = if release_after_next_tool_boundary {
        app.queue_follow_up_message_after_next_tool_boundary(prompt)
    } else {
        app.queue_follow_up_message(prompt)
    };
    let suffix = if queued > 1 {
        format!(" {queued} follow-up messages are queued.")
    } else {
        " 1 follow-up message is queued.".to_string()
    };
    app.bottom_pane.notice = Some(format!(
        "{}{suffix}",
        if release_after_next_tool_boundary {
            "Queued for after the next tool call boundary."
        } else if app.active_pending_interaction().is_some()
            && app.pending_request_input().is_none()
        {
            "Queued until the pending interaction is answered."
        } else {
            "Queued for after the current task finishes."
        }
    ));
    publish_input_event(
        app,
        InputEvent::FollowUpQueued {
            queue_len: queued as u32,
        },
    );
    InputControlOutcome::Queued
}

pub(crate) fn answer_pending_input(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    agent: Agent,
    answer: String,
) {
    publish_input_event(app, InputEvent::PendingInputAnswered);
    if app.has_local_pending_request_input() {
        start_local_request_input_continuation(app, agent, answer);
        return;
    }

    let request = crate::runtime_control::InputControlRequest::AnswerPendingInput { answer };

    start_input_control_task(
        app,
        agent,
        request,
        "Answering pending input.".into(),
        RuntimePhase::ProcessingResponse,
        Some("resuming after input".into()),
    );
    *agent_slot = None;
}

pub(crate) fn answer_plan_approval(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    approved: bool,
) -> InputControlOutcome {
    if !app.has_pending_plan_approval() {
        app.push_notice("No pending plan approval.");
        return InputControlOutcome::Rejected;
    }
    let Some(agent) = agent_slot.take() else {
        app.push_notice("Approval is still preparing. Try again.");
        return InputControlOutcome::Rejected;
    };
    start_plan_approval_resume_task(app, !approved, agent);
    InputControlOutcome::Answered
}

pub(crate) fn answer_shell_approval(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: ShellApprovalDecision,
) -> InputControlOutcome {
    let Some(interaction) = app.active_pending_interaction() else {
        app.push_notice("No pending shell approval.");
        return InputControlOutcome::Rejected;
    };
    if interaction.kind != ActivePendingInteractionKind::ShellApproval {
        app.push_notice("No pending shell approval.");
        return InputControlOutcome::Rejected;
    }
    let Some(agent) = agent_slot.take() else {
        app.push_notice("Approval is still preparing. Try again.");
        return InputControlOutcome::Rejected;
    };
    let decision = BashApprovalDecision::from(decision);
    start_pending_approval_task(app, decision, agent);
    InputControlOutcome::Answered
}

pub(crate) fn handle_session_control(
    app: &mut TuiApp,
    request: SessionControlRequest,
) -> InputControlOutcome {
    match request {
        SessionControlRequest::CancelCurrentTurn => {
            request_running_task_cancellation(app);
            if app
                .bottom_pane
                .notice
                .as_deref()
                .is_some_and(|notice| notice == "Cancellation requested.")
            {
                publish_session_event(app, SessionEvent::TurnCancelled);
                InputControlOutcome::CancelRequested
            } else {
                InputControlOutcome::Rejected
            }
        }
        SessionControlRequest::InterruptCurrentTurn => {
            request_running_task_cancellation(app);
            if app
                .bottom_pane
                .notice
                .as_deref()
                .is_some_and(|notice| notice == "Cancellation requested.")
            {
                publish_session_event(app, SessionEvent::TurnInterrupted);
                InputControlOutcome::CancelRequested
            } else {
                InputControlOutcome::Rejected
            }
        }
        _ => InputControlOutcome::Rejected,
    }
}

fn start_local_request_input_continuation(app: &mut TuiApp, agent: Agent, answer: String) {
    let Some(interaction) = app.pending_request_input().cloned() else {
        app.clear_pending_planning_suggestion();
        start_query_task(app, answer, agent);
        return;
    };

    let source = interaction
        .source
        .clone()
        .unwrap_or_else(|| "sub-agent".to_string());
    app.record_completed_interaction(
        InteractionKind::RequestInput,
        interaction.title.clone(),
        format!("Answered with: {}", answer),
        interaction.source.clone(),
    );
    app.clear_local_request_input();

    let mut prompt = format!(
        "Continue the parent task after a delegated {source} requested additional user input.\nQuestion: {}\nAnswer: {}\n\nUse the delegated result already present in the transcript as context; do not assume the child sub-agent session is still attached.",
        interaction.title, answer
    );
    if let Some(note) = interaction.note.as_deref()
        && !note.trim().is_empty()
    {
        prompt.push_str(&format!("\nContext: {}", note.trim()));
    }
    start_query_task(app, prompt, agent);
}

fn publish_input_event(app: &TuiApp, event: InputEvent) {
    if let Some(bus) = app.event_bus.as_ref() {
        bus.publish_control(RuntimeEvent::Input(event));
    }
}

fn publish_session_event(app: &TuiApp, event: SessionEvent) {
    if let Some(bus) = app.event_bus.as_ref() {
        bus.publish_control(RuntimeEvent::Session(event));
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Instant;

    use tokio::sync::mpsc;

    use super::*;
    use crate::config::ConfigManager;
    use crate::runtime_control::RuntimeEvent;
    use crate::runtime_event_bus::RuntimeEventBus;
    use crate::tui::state::{RunningTask, TaskCompletion};

    fn test_app() -> TuiApp {
        let dir = tempfile::tempdir().expect("tempdir");
        TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .expect("app")
    }

    fn mark_query_busy(app: &mut TuiApp, cancellation_token: Option<Arc<AtomicBool>>) {
        let (_sender, receiver) = mpsc::unbounded_channel();
        app.bottom_pane.running_task = Some(RunningTask {
            kind: TaskKind::Query,
            receiver,
            handle: tokio::spawn(async { std::future::pending::<TaskCompletion>().await }),
            started_at: Instant::now(),
            next_heartbeat_after_secs: 2,
            cancellation_token,
            cancellation_requested: false,
        });
    }

    #[tokio::test]
    async fn submit_follow_up_publishes_structured_input_event() {
        let mut app = test_app();
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut rx = bus.subscribe_control();
        app.event_bus = Some(bus);
        mark_query_busy(&mut app, None);

        let outcome = submit_user_prompt(&mut app, &mut None, "continue".to_string());

        assert_eq!(outcome, InputControlOutcome::Queued);
        assert_eq!(app.pending_follow_up_count(), 1);
        let event = rx.try_recv().expect("input event");
        assert!(matches!(
            event.event,
            RuntimeEvent::Input(InputEvent::FollowUpQueued { queue_len: 1 })
        ));
    }

    #[tokio::test]
    async fn cancel_current_turn_publishes_session_event_when_cancelled() {
        let mut app = test_app();
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut rx = bus.subscribe_control();
        app.event_bus = Some(bus);
        let token = Arc::new(AtomicBool::new(false));
        mark_query_busy(&mut app, Some(token.clone()));

        let outcome = handle_session_control(&mut app, SessionControlRequest::CancelCurrentTurn);

        assert_eq!(outcome, InputControlOutcome::CancelRequested);
        assert!(token.load(Ordering::SeqCst));
        let event = rx.try_recv().expect("session event");
        assert!(matches!(
            event.event,
            RuntimeEvent::Session(SessionEvent::TurnCancelled)
        ));
    }

    #[tokio::test]
    async fn cancel_current_turn_publishes_session_event_only_when_cancelled() {
        let mut app = test_app();
        let bus = Arc::new(RuntimeEventBus::new(8));
        let mut rx = bus.subscribe_control();
        app.event_bus = Some(bus);
        mark_query_busy(&mut app, None);

        let outcome = handle_session_control(&mut app, SessionControlRequest::CancelCurrentTurn);

        assert_eq!(outcome, InputControlOutcome::Rejected);
        assert!(rx.try_recv().is_err());
    }
}
