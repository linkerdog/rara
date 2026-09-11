use super::*;
use crate::{InferencePrice, InferencePriceTable};

fn usage() -> InferenceTokenUsage {
    InferenceTokenUsage {
        input_tokens: 1_000,
        output_tokens: 100,
        cache_read_tokens: Some(600),
        cache_write_tokens: Some(0),
        cache_write_5m_tokens: Some(100),
        cache_write_1h_tokens: Some(100),
    }
}

fn prices() -> InferencePriceTable {
    InferencePriceTable {
        revision: "test-prices".to_string(),
        prices: vec![InferencePrice {
            provider: "test".to_string(),
            model: "model".to_string(),
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
            cache_write_5m: 3.75,
            cache_write_1h: 6.0,
        }],
    }
}

#[test]
fn retries_are_distinct_attempts_and_failed_usage_is_charged_once() {
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let context = call.context();
    let first = context.start_attempt("test", "model");
    first.record_final_usage(usage());
    first.finish(&Err::<(), _>("stream failed"));
    let retry = context.start_attempt("test", "model");
    retry.record_final_usage(usage());
    retry.record_final_usage(usage());
    retry.finish(&Ok::<_, ()>(()));
    call.finish(&Ok::<_, ()>(()));
    drop(agent);
    let snapshot = task.snapshot();
    assert_eq!(snapshot.calls.len(), 1);
    assert_eq!(snapshot.attempts.len(), 2);
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Failed);
    assert_eq!(snapshot.attempts[1].status, InferenceStatus::Succeeded);
    assert_eq!(snapshot.attempts[0].call_id, snapshot.attempts[1].call_id);
    let cost = prices().cost(&snapshot);
    assert!(cost.complete);
    assert!((cost.known_cost_usd - 0.00651).abs() < 1e-12);
}

#[test]
fn queued_child_keeps_task_open_and_late_usage_stays_with_original_parent() {
    let task = InferenceTask::default();
    let root = task.start_agent(None);
    let root_id = root.id();
    let child = task.start_agent(Some(root_id));
    drop(root);
    assert!(!task.snapshot().is_terminal());
    let other_task = InferenceTask::default();
    let call = child.start_call(InferencePurpose::Summary);
    let attempt = call.context().start_attempt("test", "model");
    attempt.record_final_usage(usage());
    attempt.finish(&Ok::<_, ()>(()));
    call.finish(&Ok::<_, ()>(()));
    drop(child);
    assert!(task.snapshot().is_terminal());
    assert_eq!(task.snapshot().calls[0].parent_agent_id, Some(root_id));
    assert!(other_task.snapshot().attempts.is_empty());
}

#[test]
fn cancellation_retains_received_usage_and_missing_usage_is_not_free() {
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Classifier);
    let attempt = call.context().start_attempt("test", "model");
    attempt.record_usage(usage());
    drop(attempt);
    drop(call);
    let call = agent.start_call(InferencePurpose::Main);
    drop(call.context().start_attempt("test", "model"));
    drop(call);
    drop(agent);
    let snapshot = task.snapshot();
    assert_eq!(snapshot.attempts[0].usage, Some(usage()));
    assert_eq!(snapshot.attempts[0].status, InferenceStatus::Cancelled);
    let cost = prices().cost(&snapshot);
    assert!(!cost.complete);
    assert_eq!(cost.unpriced_attempts, 2);
    assert!((cost.known_cost_usd - 0.003255).abs() < 1e-12);
}

#[test]
fn uninstrumented_backends_cannot_claim_complete_cost() {
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    agent
        .start_call(InferencePurpose::Summary)
        .finish(&Ok::<_, ()>(()));
    drop(agent);
    let report = prices().cost(&task.snapshot());
    assert!(!report.complete);
    assert_eq!(report.unobserved_calls, 1);
}

#[test]
fn inconsistent_categories_missing_or_ambiguous_tariffs_are_unpriced() {
    for invalid in [
        InferenceTokenUsage {
            cache_read_tokens: None,
            ..usage()
        },
        InferenceTokenUsage {
            cache_read_tokens: Some(1_001),
            ..usage()
        },
    ] {
        let task = InferenceTask::default();
        let agent = task.start_agent(None);
        let call = agent.start_call(InferencePurpose::Main);
        let attempt = call.context().start_attempt("test", "model");
        attempt.record_final_usage(invalid);
        attempt.finish(&Ok::<_, ()>(()));
        call.finish(&Ok::<_, ()>(()));
        drop(agent);
        let cost = prices().cost(&task.snapshot());
        assert!(!cost.complete);
        assert_eq!(cost.unpriced_attempts, 1);
    }
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let attempt = call.context().start_attempt("test", "model");
    attempt.record_final_usage(usage());
    attempt.finish(&Ok::<_, ()>(()));
    call.finish(&Ok::<_, ()>(()));
    drop(agent);
    let snapshot = task.snapshot();
    let mut table = prices();
    table.prices.push(table.prices[0].clone());
    assert!(!table.cost(&snapshot).complete);
    table.prices.clear();
    assert!(!table.cost(&snapshot).complete);
}

#[test]
fn concurrent_agents_do_not_lose_attempts() {
    let task = InferenceTask::default();
    std::thread::scope(|scope| {
        for _ in 0..8 {
            let agent = task.start_agent(None);
            scope.spawn(move || {
                for _ in 0..10 {
                    let call = agent.start_call(InferencePurpose::Main);
                    let attempt = call.context().start_attempt("test", "model");
                    attempt.record_final_usage(usage());
                    attempt.finish(&Ok::<_, ()>(()));
                    call.finish(&Ok::<_, ()>(()));
                }
            });
        }
    });
    let snapshot = task.snapshot();
    assert_eq!(snapshot.attempts.len(), 80);
    assert_eq!(snapshot.calls.len(), 80);
    assert!(prices().cost(&snapshot).complete);
}
