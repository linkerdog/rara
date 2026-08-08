use crate::agent::{Agent, BashApprovalDecision};
use crate::runtime_client::RuntimeTaskServices;
use crate::runtime_control::{
    InputEvent, PlanApprovalDecision, RuntimeEvent, SessionControlRequest, SessionEvent,
    ShellApprovalDecision,
};
use crate::tui::runtime::{
    request_running_task_cancellation, start_input_control_task, start_pending_approval_task,
    start_plan_approval_resume_task, start_query_task,
    tasks::start_input_control_task_with_services,
    tasks::start_pending_approval_task_with_services,
    tasks::start_plan_approval_resume_task_with_services, tasks::start_query_task_with_services,
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
    submit_user_prompt_with_services(app, agent_slot, prompt, None)
}

pub(crate) fn submit_user_prompt_with_services(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    prompt: String,
    services: Option<RuntimeTaskServices>,
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
        // Agent slot is empty — likely a previous task crashed or is still
        // rebuilding.  Queue the user's message so it isn't lost, and trigger
        // a rebuild.  After the rebuild completes, try_start_queued_follow_up
        // will drain the queue.
        let queued = app.queue_follow_up_message(prompt);
        let suffix = if queued > 1 {
            format!(" ({queued} messages queued)")
        } else {
            String::new()
        };
        app.bottom_pane.notice = Some(format!("Agent not ready — rebuilding now{suffix}."));
        publish_input_event(
            app,
            InputEvent::FollowUpQueued {
                queue_len: queued as u32,
            },
        );
        super::runtime::start_rebuild_task(app);
        return InputControlOutcome::Queued;
    };

    if app.pending_request_input().is_some() {
        answer_pending_input_with_services(app, agent_slot, agent, prompt, services);
        return InputControlOutcome::Answered;
    }

    app.clear_pending_planning_suggestion();
    publish_input_event(app, InputEvent::UserPromptSubmitted);
    if let Some(services) = services {
        start_query_task_with_services(app, prompt, agent, services);
    } else {
        start_query_task(app, prompt, agent);
    }
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
    answer_pending_input_with_services(app, agent_slot, agent, answer, None);
}

pub(crate) fn answer_pending_input_with_services(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    agent: Agent,
    answer: String,
    services: Option<RuntimeTaskServices>,
) {
    publish_input_event(app, InputEvent::PendingInputAnswered);
    if app.has_local_pending_request_input() {
        start_local_request_input_continuation(app, agent, answer, services);
        return;
    }

    let request = crate::runtime_control::InputControlRequest::AnswerPendingInput { answer };

    if let Some(services) = services {
        start_input_control_task_with_services(
            app,
            agent,
            request,
            "Answering pending input.".into(),
            RuntimePhase::ProcessingResponse,
            Some("resuming after input".into()),
            services,
        );
    } else {
        start_input_control_task(
            app,
            agent,
            request,
            "Answering pending input.".into(),
            RuntimePhase::ProcessingResponse,
            Some("resuming after input".into()),
        );
    }
    *agent_slot = None;
}

pub(crate) fn answer_plan_approval(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: PlanApprovalDecision,
) -> InputControlOutcome {
    answer_plan_approval_with_feedback(app, agent_slot, decision, None)
}

pub(crate) fn answer_plan_approval_with_feedback(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: PlanApprovalDecision,
    feedback: Option<String>,
) -> InputControlOutcome {
    answer_plan_approval_with_feedback_and_services(app, agent_slot, decision, feedback, None)
}

pub(crate) fn answer_plan_approval_with_feedback_and_services(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: PlanApprovalDecision,
    feedback: Option<String>,
    services: Option<RuntimeTaskServices>,
) -> InputControlOutcome {
    if !app.has_pending_plan_approval() {
        app.push_notice("No pending plan approval.");
        return InputControlOutcome::Rejected;
    }
    let Some(agent) = agent_slot.take() else {
        app.push_notice("Approval is still preparing. Try again.");
        return InputControlOutcome::Rejected;
    };
    let (summary, notice) = match decision {
        PlanApprovalDecision::Approve => (
            "Approved. Starting implementation.",
            "Plan approved. Continuing with implementation.",
        ),
        PlanApprovalDecision::ContinuePlanning => (
            "Sent back for more planning.",
            "Continuing plan refinement.",
        ),
        PlanApprovalDecision::Reject => (
            "Rejected. Implementation cancelled.",
            "Plan rejected. Implementation cancelled.",
        ),
    };
    let source = Some(plan_approval_decision_source(decision).to_string());
    let feedback = feedback
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    if decision == PlanApprovalDecision::Reject {
        let mut agent = agent;
        if let Err(err) = agent.reject_pending_plan_approval(feedback.as_deref()) {
            app.push_notice(format!("Failed to record plan rejection: {err}"));
            *agent_slot = Some(agent);
            return InputControlOutcome::Rejected;
        }
        app.clear_pending_plan_approval();
        app.record_completed_interaction_with_metadata(
            InteractionKind::PlanApproval,
            "Plan Decision",
            summary,
            source.clone(),
            feedback,
            None,
        );
        app.set_agent_execution_mode(agent.execution_mode);
        app.bottom_pane.notice = Some(notice.to_string());
        app.set_runtime_phase(RuntimePhase::Idle, Some("plan cancelled".into()));
        *agent_slot = Some(agent);
        return InputControlOutcome::Answered;
    }

    app.clear_pending_plan_approval();
    let plan_revision = if decision == PlanApprovalDecision::Approve {
        Some(agent.current_plan_hash())
    } else {
        None
    };
    app.record_completed_interaction_with_metadata(
        InteractionKind::PlanApproval,
        "Plan Decision",
        summary,
        source,
        feedback.clone(),
        plan_revision,
    );
    if let Some(services) = services {
        start_plan_approval_resume_task_with_services(app, decision, feedback, agent, services);
    } else {
        start_plan_approval_resume_task(app, decision, feedback, agent);
    }
    InputControlOutcome::Answered
}

pub(crate) fn plan_approval_decision_source(decision: PlanApprovalDecision) -> &'static str {
    match decision {
        PlanApprovalDecision::Approve => "plan_approval:approve",
        PlanApprovalDecision::ContinuePlanning => "plan_approval:continue_planning",
        PlanApprovalDecision::Reject => "plan_approval:reject",
    }
}

pub(crate) fn plan_approval_decision_for_index(index: usize) -> Option<PlanApprovalDecision> {
    match index {
        0 => Some(PlanApprovalDecision::Approve),
        1 => Some(PlanApprovalDecision::ContinuePlanning),
        2 => Some(PlanApprovalDecision::Reject),
        _ => None,
    }
}

pub(crate) fn answer_shell_approval(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: ShellApprovalDecision,
) -> InputControlOutcome {
    answer_shell_approval_with_services(app, agent_slot, decision, None)
}

pub(crate) fn answer_shell_approval_with_services(
    app: &mut TuiApp,
    agent_slot: &mut Option<Agent>,
    decision: ShellApprovalDecision,
    services: Option<RuntimeTaskServices>,
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
    if let Some(services) = services {
        start_pending_approval_task_with_services(app, decision, agent, services);
    } else {
        start_pending_approval_task(app, decision, agent);
    }
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

fn start_local_request_input_continuation(
    app: &mut TuiApp,
    agent: Agent,
    answer: String,
    services: Option<RuntimeTaskServices>,
) {
    let Some(interaction) = app.pending_request_input().cloned() else {
        app.clear_pending_planning_suggestion();
        if let Some(services) = services {
            start_query_task_with_services(app, answer, agent, services);
        } else {
            start_query_task(app, answer, agent);
        }
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
    if let Some(services) = services {
        start_query_task_with_services(app, prompt, agent, services);
    } else {
        start_query_task(app, prompt, agent);
    }
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

    #[tokio::test]
    async fn absent_agent_queues_prompt_and_starts_rebuild() {
        let mut app = test_app();
        // agent_slot is None by default — test_app() has no agent.

        let outcome = submit_user_prompt(&mut app, &mut None, "hello".to_string());

        // Should accept the input (not reject) and queue the message.
        assert_eq!(outcome, InputControlOutcome::Queued);
        assert_eq!(app.queued_follow_up_count(), 1, "message should be queued");

        // Should have started a Rebuild task since none was running.
        let task = app.bottom_pane.running_task.as_ref().expect("rebuild task");
        assert!(
            matches!(task.kind, TaskKind::Rebuild),
            "expected Rebuild task, got {:?}",
            task.kind
        );
    }

    #[tokio::test]
    async fn absent_agent_during_rebuild_queues_without_duplicate_rebuild() {
        let mut app = test_app();
        // Simulate a rebuild already in progress.
        let (_sender, receiver) = mpsc::unbounded_channel();
        app.bottom_pane.running_task = Some(RunningTask {
            kind: TaskKind::Rebuild,
            receiver,
            handle: tokio::spawn(async { std::future::pending::<TaskCompletion>().await }),
            started_at: Instant::now(),
            next_heartbeat_after_secs: 2,
            cancellation_token: None,
            cancellation_requested: false,
        });

        let outcome = submit_user_prompt(&mut app, &mut None, "hello".to_string());

        assert_eq!(outcome, InputControlOutcome::Queued);
        assert_eq!(app.queued_follow_up_count(), 1);
        // The task should still be Rebuild (not replaced).
        let task = app
            .bottom_pane
            .running_task
            .as_ref()
            .expect("task still present");
        assert!(
            matches!(task.kind, TaskKind::Rebuild),
            "should not replace existing Rebuild"
        );
    }
}
