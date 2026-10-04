#![cfg(all(target_arch = "wasm32", target_os = "unknown"))]

use rara_observability::{
    InferencePurpose, InferenceStatus, InferenceTask, MemoryObservability, MemoryOperation,
};
use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_browser);

#[wasm_bindgen_test]
fn browser_inference_accounting_records_success_failure_and_drop() {
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    for outcome in [Some(Ok::<(), ()>(())), Some(Err(())), None] {
        let call = agent.start_call(InferencePurpose::Main);
        let attempt = call.context().start_attempt("browser", "local");
        if let Some(result) = outcome {
            attempt.finish(&result);
            call.finish(&result);
        }
    }
    drop(agent);
    let snapshot = task.snapshot();
    assert_eq!(snapshot.active_agents, 0);
    let expected = vec![
        InferenceStatus::Succeeded,
        InferenceStatus::Failed,
        InferenceStatus::Cancelled,
    ];
    assert_eq!(
        snapshot
            .calls
            .iter()
            .map(|call| call.status)
            .collect::<Vec<_>>(),
        expected
    );
    assert_eq!(
        snapshot
            .attempts
            .iter()
            .map(|attempt| attempt.status)
            .collect::<Vec<_>>(),
        expected
    );
}

#[wasm_bindgen_test]
fn browser_memory_timer_records_a_sample() {
    let memory = MemoryObservability::new(4);
    drop(memory.start_timer(MemoryOperation::Read));
    assert_eq!(memory.snapshot().read.sample_count, 1);
}
