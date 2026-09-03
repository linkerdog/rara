use super::*;
use crate::agent::AgentEvent;

fn record(agent_id: &str, parent_session_id: &str) -> BackgroundSubAgentRecord {
    BackgroundSubAgentRecord {
        agent_id: agent_id.to_string(),
        path: format!("/root/{agent_id}"),
        session_id: format!("session-{agent_id}"),
        name: Some(agent_id.to_string()),
        provider: None,
        model: None,
        progress: SubagentProgress::new(agent_id),
        kind: "explore",
        parent_session_id: Some(parent_session_id.to_string()),
        status: "running".to_string(),
        summary: None,
        error: None,
        persistence_error: None,
        plan: None,
        plan_explanation: None,
        request_user_input: None,
        started_at: 1,
        finished_at: None,
    }
}

fn completed_result(agent_id: &str) -> SubAgentResult {
    SubAgentResult {
        agent_id: agent_id.to_string(),
        session_id: format!("session-{agent_id}"),
        status: "explored",
        summary: "finished".to_string(),
        provider: "mock".to_string(),
        model: "mock-model".to_string(),
        persistence_error: None,
        plan: None,
        plan_explanation: None,
        request_user_input: None,
        total_input_tokens: 10,
        total_output_tokens: 5,
        total_cache_hit_tokens: 2,
        total_cache_miss_tokens: 8,
        token_budget: None,
        token_budget_exhausted: false,
    }
}

#[test]
fn records_are_scoped_to_the_parent_session() {
    let control = AgentTreeControl::default();
    let mut inner = control.inner.lock().expect("control");
    inner
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));
    inner
        .tasks
        .insert("agent-b".to_string(), record("agent-b", "parent-b"));
    drop(inner);

    let visible = control.list_for_parent("parent-a").expect("list");
    assert_eq!(visible.len(), 1);
    assert_eq!(visible[0].agent_id, "agent-a");
    assert!(control.get_for_parent("agent-b", "parent-a").is_err());
    assert!(control.get_for_parent("/root/agent-a", "parent-a").is_ok());
}

#[test]
fn completion_is_enqueued_exactly_once() {
    let control = AgentTreeControl::default();
    control
        .inner
        .lock()
        .expect("control")
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));
    let result = Ok(completed_result("agent-a"));

    control.finish("agent-a", &result, AgentResultDelivery::Mailbox);
    control.finish("agent-a", &result, AgentResultDelivery::Mailbox);

    let messages = control.drain_mailbox("parent-a");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].sender_agent_id.as_deref(), Some("agent-a"));
    assert_eq!(messages[0].kind, "completion");
    assert!(messages[0].payload.contains("mock-model"));
}

#[test]
fn resolved_model_replaces_requested_route_metadata() {
    let control = AgentTreeControl::default();
    control
        .inner
        .lock()
        .expect("control")
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));

    control
        .record_model_resolution("agent-a", "gemini", "gemini-2.5-pro")
        .expect("model resolution");

    let record = control
        .get_for_parent("agent-a", "parent-a")
        .expect("record");
    assert_eq!(record.provider.as_deref(), Some("gemini"));
    assert_eq!(record.model.as_deref(), Some("gemini-2.5-pro"));
}

#[test]
fn typed_events_update_bounded_agent_activity_snapshot() {
    let control = AgentTreeControl::default();
    control
        .inner
        .lock()
        .expect("control")
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));

    control
        .record_progress_event("agent-a", &AgentEvent::Status("Searching workspace".into()))
        .expect("status progress");
    control
        .record_progress_event(
            "agent-a",
            &AgentEvent::ToolUse {
                call_id: "call-1".into(),
                name: "grep".into(),
                input: json!({"pattern": "AgentTreeControl"}),
            },
        )
        .expect("tool progress");
    control
        .record_progress_event(
            "agent-a",
            &AgentEvent::ModelRequest {
                model: "mock-model".into(),
                input_tokens: 12,
            },
        )
        .expect("input tokens");
    control
        .record_progress_event(
            "agent-a",
            &AgentEvent::ModelResponse {
                model: "mock-model".into(),
                output_tokens: 7,
                finish_reason: Some("tool_use".into()),
            },
        )
        .expect("output tokens");

    let snapshots = control
        .activity_snapshots_for_root("parent-a")
        .expect("snapshots");
    assert_eq!(snapshots.len(), 1);
    assert_eq!(snapshots[0].tool_use_count, 1);
    assert_eq!(snapshots[0].total_tokens, 19);
    assert_eq!(snapshots[0].latest_activity.as_deref(), Some("Using grep"));

    control
        .record_progress_event("agent-a", &AgentEvent::Status("x".repeat(200)))
        .expect("bounded status");
    let bounded = control
        .activity_snapshots_for_root("parent-a")
        .expect("bounded snapshot")[0]
        .latest_activity
        .clone()
        .expect("activity");
    assert_eq!(bounded.chars().count(), 121);
    assert!(bounded.ends_with('…'));
}

#[test]
fn activity_projection_includes_descendants_but_not_other_roots() {
    let control = AgentTreeControl::default();
    let mut inner = control.inner.lock().expect("control");
    inner
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "root-a"));
    inner.tasks.insert(
        "agent-a-child".to_string(),
        record("agent-a-child", "session-agent-a"),
    );
    inner
        .tasks
        .insert("agent-b".to_string(), record("agent-b", "root-b"));
    drop(inner);

    let snapshots = control
        .activity_snapshots_for_root("root-a")
        .expect("snapshots");
    let ids = snapshots
        .iter()
        .map(|snapshot| snapshot.agent_id.as_str())
        .collect::<HashSet<_>>();
    assert_eq!(ids, HashSet::from(["agent-a", "agent-a-child"]));
}

#[test]
fn parent_messages_are_ordered_and_child_scoped() {
    let control = AgentTreeControl::default();
    control
        .inner
        .lock()
        .expect("control")
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));

    control
        .send_to_child("parent-a", "agent-a", "message", "first".to_string())
        .expect("first message");
    control
        .send_to_child("parent-a", "agent-a", "followup", "second".to_string())
        .expect("second message");
    assert!(
        control
            .send_to_child("parent-b", "agent-a", "message", "hidden".to_string())
            .is_err()
    );

    let messages = control.drain_mailbox("session-agent-a");
    assert_eq!(messages.len(), 2);
    assert!(messages[0].sequence < messages[1].sequence);
    assert_eq!(messages[0].payload, "first");
    assert_eq!(messages[1].payload, "second");
    assert!(control.drain_mailbox("session-agent-a").is_empty());
}

#[test]
fn active_capacity_is_shared_by_one_control() {
    let control = AgentTreeControl::new(AgentTreeConfig::new(
        NonZeroUsize::new(2).expect("positive capacity"),
    ));
    let first = control.active.clone().try_acquire_owned().expect("first");
    let second = control.active.clone().try_acquire_owned().expect("second");
    assert_eq!(control.available_permits(), 0);
    assert!(control.active.clone().try_acquire_owned().is_err());
    drop(first);
    assert_eq!(control.available_permits(), 1);
    drop(second);
    assert_eq!(control.available_permits(), 2);
}

#[tokio::test]
async fn wait_returns_new_mailbox_activity() {
    let control = Arc::new(AgentTreeControl::default());
    control
        .inner
        .lock()
        .expect("control")
        .tasks
        .insert("agent-a".to_string(), record("agent-a", "parent-a"));
    let sender = control.clone();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        sender
            .send_to_child("parent-a", "agent-a", "message", "wake".to_string())
            .expect("send");
    });

    let (messages, timed_out) = control
        .wait_for_messages("session-agent-a", None, Duration::from_millis(100))
        .await
        .expect("wait");
    assert!(!timed_out);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].payload, "wake");
}

#[tokio::test]
async fn targeted_wait_ignores_unrelated_activity() {
    let control = Arc::new(AgentTreeControl::default());
    {
        let mut inner = control.inner.lock().expect("control");
        inner
            .tasks
            .insert("agent-a".to_string(), record("agent-a", "parent-a"));
        inner
            .tasks
            .insert("agent-b".to_string(), record("agent-b", "parent-a"));
    }

    let sender = control.clone();
    tokio::spawn(async move {
        sender.finish(
            "agent-b",
            &Ok(completed_result("agent-b")),
            AgentResultDelivery::Mailbox,
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
        sender.finish(
            "agent-a",
            &Ok(completed_result("agent-a")),
            AgentResultDelivery::Mailbox,
        );
    });
    let targets = HashSet::from(["agent-a".to_string()]);

    let (messages, timed_out) = control
        .wait_for_messages("parent-a", Some(&targets), Duration::from_millis(100))
        .await
        .expect("wait");

    assert!(!timed_out);
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].sender_agent_id.as_deref(), Some("agent-a"));
    let retained = control.drain_mailbox("parent-a");
    assert_eq!(retained.len(), 1);
    assert_eq!(retained[0].sender_agent_id.as_deref(), Some("agent-b"));
}

#[tokio::test]
async fn shutdown_cancels_and_waits_for_active_children() {
    let control = Arc::new(AgentTreeControl::default());
    let cancellation = Arc::new(AtomicBool::new(false));
    {
        let mut inner = control.inner.lock().expect("control");
        inner
            .tasks
            .insert("agent-a".to_string(), record("agent-a", "parent-a"));
        inner
            .cancellations
            .insert("agent-a".to_string(), cancellation.clone());
        inner.active_tasks.insert("agent-a".to_string());
    }

    let shutdown_control = control.clone();
    let shutdown = tokio::spawn(async move { shutdown_control.shutdown().await });
    tokio::time::timeout(Duration::from_secs(1), async {
        while !cancellation.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("child cancellation");
    assert!(cancellation.load(Ordering::SeqCst));
    assert!(!shutdown.is_finished());

    control.finish(
        "agent-a",
        &Ok(completed_result("agent-a")),
        AgentResultDelivery::Direct,
    );
    shutdown.await.expect("shutdown task").expect("shutdown");
    let inner = control.inner.lock().expect("control");
    assert!(inner.closing);
    assert!(inner.active_tasks.is_empty());
}
