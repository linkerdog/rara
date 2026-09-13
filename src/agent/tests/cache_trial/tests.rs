use std::sync::atomic::{AtomicUsize, Ordering};

use rara_observability::{InferencePrice, InferencePurpose, InferenceStatus, InferenceTokenUsage};

use super::*;
use crate::llm::{
    ContentBlock, LlmResponse, LlmStreamEvent, LlmTurnMetadata, Message, SummaryPrefix,
};

const REPAIR: &str = "def stable_unique(records):\n    result = []\n    seen = set()\n    for record in records:\n        if record['id'] not in seen:\n            seen.add(record['id'])\n            result.append(record)\n    return result\n\ndef merge_unique(existing, incoming):\n    return stable_unique(existing + incoming)\n";

struct FixtureBackend {
    calls: AtomicUsize,
    receipt: Receipt,
}

#[derive(Clone, Copy)]
enum Receipt {
    Complete,
    Missing,
    Pending,
}

fn prices() -> InferencePriceTable {
    InferencePriceTable {
        revision: "fixture-only".into(),
        prices: vec![InferencePrice {
            provider: "DeepSeek".into(),
            model: "deepseek-v4-pro".into(),
            input: 1.0,
            output: 1.0,
            cache_read: 0.1,
            cache_write: 0.0,
            cache_write_5m: 0.0,
            cache_write_1h: 0.0,
        }],
    }
}

async fn record(metadata: &LlmTurnMetadata, receipt: Receipt) {
    let attempt = metadata
        .start_attempt("DeepSeek", "deepseek-v4-pro")
        .expect("accounting");
    match receipt {
        Receipt::Missing => {
            attempt.finish(&Ok::<_, ()>(()));
            return;
        }
        Receipt::Pending => std::future::pending::<()>().await,
        Receipt::Complete => {}
    }
    attempt.record_final_usage(InferenceTokenUsage {
        input_tokens_incomplete: false,
        input_tokens: 100,
        output_tokens: 10,
        cache_read_tokens: Some(80),
        cache_write_tokens: Some(0),
        cache_write_5m_tokens: Some(0),
        cache_write_1h_tokens: Some(0),
    });
    attempt.finish(&Ok::<_, ()>(()));
}

#[async_trait::async_trait]
impl LlmBackend for FixtureBackend {
    async fn ask(&self, _: &[Message], _: &[Value]) -> Result<LlmResponse> {
        anyhow::bail!("metadata required")
    }
    async fn summarize(&self, _: &[Message], _: &str) -> Result<String> {
        anyhow::bail!("metadata required")
    }
    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        _: &[Value],
        metadata: LlmTurnMetadata,
        _: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        record(&metadata, self.receipt).await;
        let last = messages.last().expect("message");
        let text = last.content.to_string();
        let repair = if text.contains("Leave merge_unique for the next task") {
            REPAIR.replace(
                "return stable_unique(existing + incoming)",
                "raise NotImplementedError",
            )
        } else {
            REPAIR.to_owned()
        };
        let content = if text.contains("tool_result") || text.contains("Review both helpers") {
            vec![ContentBlock::Text {
                text: "checked".into(),
            }]
        } else {
            vec![ContentBlock::ToolUse {
                id: format!("write-{}", self.calls.load(Ordering::SeqCst)),
                name: "write_file".into(),
                input: json!({"path":"task.py", "content":repair}),
            }]
        };
        Ok(LlmResponse {
            content,
            stop_reason: Some("end_turn".into()),
            usage: None,
        })
    }
    async fn summarize_with_context(
        &self,
        _: &[Message],
        _: &str,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        // Exercise the real driver's timing policy instead of returning within
        // the fast compaction deadline used by ordinary unit tests.
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        self.calls.fetch_add(1, Ordering::SeqCst);
        record(&metadata, self.receipt).await;
        Ok("Preserve POLICY.md: exact case-sensitive IDs, first-wins encounter order, original object identity. The first task repaired stable_unique. Next implement merge_unique, then review.".into())
    }
    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        prefix: &SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        prefix.messages_for_summary(messages, instruction)?;
        self.summarize_with_context(messages, instruction, metadata)
            .await
    }
}

#[tokio::test]
async fn insufficient_budget_never_polls_the_provider() {
    let prices = prices();
    let task = InferenceTask::default();
    let agent = task.start_agent(None);
    let call = agent.start_call(InferencePurpose::Main);
    let inner = Arc::new(FixtureBackend {
        calls: AtomicUsize::new(0),
        receipt: Receipt::Complete,
    });
    let backend = CappedBackend {
        inner: inner.clone(),
        task: task.clone(),
        budget: Arc::new(Mutex::new(Budget::new(0.01, &prices, 100).unwrap())),
        prices,
    };
    let result = backend
        .ask_streaming_with_context(
            &[],
            &[],
            LlmTurnMetadata::default().with_inference(call.context()),
            &mut |_| {},
        )
        .await;
    assert!(result.is_err());
    assert_eq!(inner.calls.load(Ordering::SeqCst), 0);
    assert!(task.snapshot().attempts.is_empty());
}

#[tokio::test]
async fn missing_or_cancelled_receipts_block_all_later_provider_calls() {
    for receipt in [Receipt::Missing, Receipt::Pending] {
        let prices = prices();
        let task = InferenceTask::default();
        let agent = task.start_agent(None);
        let inner = Arc::new(FixtureBackend {
            calls: AtomicUsize::new(0),
            receipt,
        });
        // This ceiling has room for another full reservation. Only the unsettled
        // receipt, rather than exhausted funds, must block the second call.
        let backend = CappedBackend {
            inner: inner.clone(),
            task: task.clone(),
            budget: Arc::new(Mutex::new(Budget::new(30.0, &prices, 100).unwrap())),
            prices,
        };
        for _ in 0..2 {
            let call = agent.start_call(InferencePurpose::Main);
            let metadata = LlmTurnMetadata::default().with_inference(call.context());
            let mut events = |_| {};
            let messages = [Message {
                role: "user".into(),
                content: json!("Review both helpers"),
            }];
            let result = tokio::time::timeout(
                std::time::Duration::from_millis(20),
                backend.ask_streaming_with_context(&messages, &[], metadata, &mut events),
            )
            .await;
            assert!(!matches!(result, Ok(Ok(_))));
        }
        assert_eq!(inner.calls.load(Ordering::SeqCst), 1);
        let snapshot = task.snapshot();
        assert_eq!(snapshot.attempts.len(), 1);
        assert!(!backend.prices.cost(&snapshot).complete);
        if matches!(receipt, Receipt::Pending) {
            assert_eq!(snapshot.attempts[0].status, InferenceStatus::Cancelled);
        }
    }
}

#[tokio::test]
async fn offline_driver_preserves_tools_accounting_and_grading_availability() {
    let python = Path::new("python3");
    // Nested Seatbelt and unsupported hosts must keep quality unknown. The
    // standalone Python calibration requires a working sandbox and real grades.
    let sandbox = python_receipt(python, &["preflight".into()]).await.unwrap();
    let sandbox_available = sandbox["sandbox_available"].as_bool().unwrap();
    let expected_grade = sandbox_available.then_some(true);
    let corpus: Corpus = serde_json::from_value(
        python_receipt(python, &["export".into(), "--case".into(), "3".into()])
            .await
            .unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let prices = prices();
    let budget = Arc::new(Mutex::new(Budget::new(9.0, &prices, 100).unwrap()));
    let mut samples = Vec::new();
    for arm in [
        InferenceExperimentArm::Baseline,
        InferenceExperimentArm::Candidate,
    ] {
        let task = InferenceTask::default();
        let backend = Arc::new(CappedBackend {
            inner: Arc::new(FixtureBackend {
                calls: AtomicUsize::new(0),
                receipt: Receipt::Complete,
            }),
            task: task.clone(),
            prices: prices.clone(),
            budget: budget.clone(),
        });
        let root = temp.path().join(format!("{arm:?}"));
        let (sample, detail) = run_case(CaseRun {
            case: &corpus.cases[0],
            corpus_sha256: &corpus.corpus_sha256,
            root: &root,
            python,
            backend,
            accounting: task,
            arm,
            comparison: Comparison::Summary,
            repetition: 0,
        })
        .await
        .unwrap();
        assert_eq!(sample.passed, expected_grade, "{detail}");
        let grades = detail["grades"].as_array().unwrap();
        assert_eq!(grades.len(), 2);
        for grade in grades {
            assert_eq!(grade["passed"].as_bool(), expected_grade, "{grade}");
            if !sandbox_available {
                assert!(matches!(
                    grade["reason"].as_str(),
                    Some("grader_unavailable" | "invalid_grader_receipt")
                ));
            }
        }
        assert_eq!(detail["compactions"], 1);
        assert_eq!(
            sample
                .accounting
                .calls
                .iter()
                .filter(|call| call.purpose == InferencePurpose::Summary)
                .count(),
            1
        );
        assert!(sample.accounting.tool_requests >= 2);
        assert!(prices.cost(&sample.accounting).complete);
        let artifact = serde_json::to_string(&json!({"sample":sample,"detail":detail})).unwrap();
        assert!(!artifact.contains(REPAIR));
        assert!(!artifact.contains(&root.display().to_string()));
        samples.push(sample);
    }
    let comparison = prices.compare_tasks(&samples);
    assert!(comparison.paired);
    assert_eq!(comparison.quality_preserved, expected_grade);
    assert!(comparison.baseline.cost_complete && comparison.candidate.cost_complete);
}

#[tokio::test]
async fn grader_failure_retains_already_incurred_task_costs() {
    let corpus: Corpus = serde_json::from_value(
        python_receipt(
            Path::new("python3"),
            &["export".into(), "--case".into(), "3".into()],
        )
        .await
        .unwrap(),
    )
    .unwrap();
    let temp = tempfile::tempdir().unwrap();
    let task = InferenceTask::default();
    let (sample, detail) = run_case(CaseRun {
        case: &corpus.cases[0],
        corpus_sha256: &corpus.corpus_sha256,
        root: &temp.path().join("trial"),
        python: &temp.path().join("missing-python"),
        backend: Arc::new(FixtureBackend {
            calls: AtomicUsize::new(0),
            receipt: Receipt::Complete,
        }),
        accounting: task,
        arm: InferenceExperimentArm::Baseline,
        comparison: Comparison::Summary,
        repetition: 0,
    })
    .await
    .unwrap();
    assert_eq!(sample.passed, None);
    assert_eq!(detail["failure_stage"], "grader_receipt");
    let cost = prices().cost(&sample.accounting);
    assert!(cost.complete);
    assert!(cost.known_cost_usd > 0.0);
}

#[test]
fn tariff_and_budget_validation_rejects_unbounded_inputs() {
    for limit in [0.0, -1.0, f64::NAN, f64::INFINITY, 101.0] {
        assert!(Budget::new(limit, &prices(), 100).is_err());
    }
    let mut invalid = prices();
    invalid.prices[0].output = f64::NAN;
    assert!(Budget::new(9.0, &invalid, 100).is_err());

    let window = TariffWindow {
        valid_from_unix_ms: 1_000,
        valid_until_unix_ms: 100_000,
    };
    for milliseconds in [0, 10_000, 100_000, 110_000] {
        assert!(
            window
                .check_at(UNIX_EPOCH + std::time::Duration::from_millis(milliseconds))
                .is_err()
        );
    }
    assert!(
        window
            .check_at(UNIX_EPOCH + std::time::Duration::from_millis(1_000))
            .is_ok()
    );
}
