use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::{InferencePriceTable, InferenceSnapshot, InferenceStatus};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceExperimentArm {
    Baseline,
    Candidate,
}

/// One externally graded task, after all foreground and background work drains.
/// Case identities are opaque numeric fixture IDs, never prompt text or paths.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceExperimentSample {
    pub case_id: u64,
    pub repetition: u32,
    pub arm: InferenceExperimentArm,
    pub passed: Option<bool>,
    pub duration_ms: u64,
    pub accounting: InferenceSnapshot,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct InferenceArmReport {
    pub tasks: usize,
    pub graded_tasks: usize,
    pub passed_tasks: usize,
    pub known_cost_usd: f64,
    pub mean_cost_usd: Option<f64>,
    /// Failed tasks are charged in the numerator too.
    pub cost_per_passed_task_usd: Option<f64>,
    pub request_count: usize,
    pub failed_or_cancelled_requests: usize,
    pub unpriced_requests: usize,
    pub unobserved_calls: usize,
    pub known_input_tokens: u64,
    pub known_output_tokens: u64,
    pub known_cache_read_tokens: u64,
    pub known_cache_write_tokens: u64,
    pub tool_requests: u64,
    pub rejected_tool_requests: u64,
    pub p50_task_duration_ms: Option<u64>,
    pub p95_task_duration_ms: Option<u64>,
    pub cost_complete: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct InferenceComparisonReport {
    pub price_revision: String,
    pub baseline: InferenceArmReport,
    pub candidate: InferenceArmReport,
    pub paired: bool,
    pub quality_preserved: Option<bool>,
    /// Point estimate only; this is not a statistical confidence interval.
    pub savings_fraction: Option<f64>,
}

impl InferencePriceTable {
    pub fn compare_tasks(
        &self,
        samples: &[InferenceExperimentSample],
    ) -> InferenceComparisonReport {
        let baseline = summarize(self, samples, InferenceExperimentArm::Baseline);
        let candidate = summarize(self, samples, InferenceExperimentArm::Candidate);
        let mut pairs = BTreeMap::<_, [Option<&InferenceExperimentSample>; 2]>::new();
        let mut duplicate = false;
        for sample in samples {
            let index = match sample.arm {
                InferenceExperimentArm::Baseline => 0,
                InferenceExperimentArm::Candidate => 1,
            };
            let pair = pairs
                .entry((sample.case_id, sample.repetition))
                .or_default();
            duplicate |= pair[index].replace(sample).is_some();
        }
        let paired = !duplicate
            && !pairs.is_empty()
            && pairs.values().all(|pair| pair.iter().all(Option::is_some));
        let graded = paired && samples.iter().all(|sample| sample.passed.is_some());
        let quality_preserved = graded.then(|| {
            pairs.values().all(|pair| {
                let baseline = pair[0].expect("paired").passed.expect("graded");
                let candidate = pair[1].expect("paired").passed.expect("graded");
                !baseline || candidate
            })
        });
        let savings_fraction = (paired
            && baseline.cost_complete
            && candidate.cost_complete
            && baseline.known_cost_usd > 0.0)
            .then(|| 1.0 - candidate.known_cost_usd / baseline.known_cost_usd);
        InferenceComparisonReport {
            price_revision: self.revision.clone(),
            baseline,
            candidate,
            paired,
            quality_preserved,
            savings_fraction,
        }
    }
}

fn summarize(
    table: &InferencePriceTable,
    samples: &[InferenceExperimentSample],
    arm: InferenceExperimentArm,
) -> InferenceArmReport {
    let mut report = InferenceArmReport {
        cost_complete: true,
        ..Default::default()
    };
    let mut durations = Vec::new();
    for sample in samples.iter().filter(|sample| sample.arm == arm) {
        report.tasks += 1;
        report.graded_tasks += usize::from(sample.passed.is_some());
        report.passed_tasks += usize::from(sample.passed == Some(true));
        durations.push(sample.duration_ms);
        let cost = table.cost(&sample.accounting);
        let total = report.known_cost_usd + cost.known_cost_usd;
        if total.is_finite() {
            report.known_cost_usd = total;
        }
        report.cost_complete &= cost.complete && total.is_finite();
        report.unpriced_requests += cost.unpriced_attempts;
        report.unobserved_calls += cost.unobserved_calls;
        report.tool_requests = report
            .tool_requests
            .saturating_add(sample.accounting.tool_requests);
        report.rejected_tool_requests = report
            .rejected_tool_requests
            .saturating_add(sample.accounting.rejected_tool_requests);
        for attempt in &sample.accounting.attempts {
            report.request_count += 1;
            report.failed_or_cancelled_requests += usize::from(matches!(
                attempt.status,
                InferenceStatus::Failed | InferenceStatus::Cancelled
            ));
            if let Some(usage) = attempt.usage {
                report.known_input_tokens =
                    report.known_input_tokens.saturating_add(usage.input_tokens);
                report.known_output_tokens = report
                    .known_output_tokens
                    .saturating_add(usage.output_tokens);
                report.known_cache_read_tokens = report
                    .known_cache_read_tokens
                    .saturating_add(usage.cache_read_tokens.unwrap_or(0));
                for writes in [
                    usage.cache_write_tokens,
                    usage.cache_write_5m_tokens,
                    usage.cache_write_1h_tokens,
                ]
                .into_iter()
                .flatten()
                {
                    report.known_cache_write_tokens =
                        report.known_cache_write_tokens.saturating_add(writes);
                }
            }
        }
    }
    report.cost_complete &= report.tasks > 0;
    if report.cost_complete {
        report.mean_cost_usd = Some(report.known_cost_usd / report.tasks as f64);
        if report.graded_tasks == report.tasks && report.passed_tasks > 0 {
            report.cost_per_passed_task_usd =
                Some(report.known_cost_usd / report.passed_tasks as f64);
        }
    }
    durations.sort_unstable();
    if !durations.is_empty() {
        report.p50_task_duration_ms = Some(durations[(durations.len() * 50).div_ceil(100) - 1]);
        report.p95_task_duration_ms = Some(durations[(durations.len() * 95).div_ceil(100) - 1]);
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{InferencePrice, InferencePurpose, InferenceTask, InferenceTokenUsage};

    fn table() -> InferencePriceTable {
        InferencePriceTable {
            revision: "fixture".into(),
            prices: vec![InferencePrice {
                provider: "fixture".into(),
                model: "main".into(),
                input: 1.0,
                output: 1.0,
                cache_read: 0.1,
                cache_write: 1.25,
                cache_write_5m: 1.25,
                cache_write_1h: 2.0,
            }],
        }
    }

    fn sample(
        arm: InferenceExperimentArm,
        case_id: u64,
        passed: Option<bool>,
        output: u64,
    ) -> InferenceExperimentSample {
        let task = InferenceTask::default();
        let agent = task.start_agent(None);
        let call = agent.start_call(InferencePurpose::Main);
        let attempt = call.context().start_attempt("fixture", "main");
        attempt.record_final_usage(InferenceTokenUsage {
            input_tokens: 100,
            output_tokens: output,
            cache_read_tokens: Some(80),
            cache_write_tokens: Some(0),
            cache_write_5m_tokens: Some(0),
            cache_write_1h_tokens: Some(0),
        });
        attempt.finish(&Ok::<_, ()>(()));
        call.finish(&Ok::<_, ()>(()));
        drop(agent);
        InferenceExperimentSample {
            case_id,
            repetition: 0,
            arm,
            passed,
            duration_ms: 100,
            accounting: task.snapshot(),
        }
    }

    #[test]
    fn task_failures_still_cost_money_and_quality_regression_stays_visible() {
        let report = table().compare_tasks(&[
            sample(InferenceExperimentArm::Baseline, 1, Some(true), 100),
            sample(InferenceExperimentArm::Baseline, 2, Some(true), 100),
            sample(InferenceExperimentArm::Candidate, 1, Some(false), 1),
            sample(InferenceExperimentArm::Candidate, 2, Some(true), 1),
        ]);
        assert!(report.savings_fraction.unwrap() > 0.0);
        assert_eq!(report.quality_preserved, Some(false));
        assert_eq!(
            report.candidate.cost_per_passed_task_usd,
            Some(report.candidate.known_cost_usd)
        );
    }

    #[test]
    fn aggregate_cost_overflow_keeps_a_finite_incomplete_report() {
        let mut prices = table();
        prices.prices[0].output = 1e308;
        let samples = [
            sample(InferenceExperimentArm::Baseline, 1, Some(true), 1_000_000),
            sample(InferenceExperimentArm::Baseline, 2, Some(true), 1_000_000),
        ];
        let report = prices.compare_tasks(&samples);
        assert!(report.baseline.known_cost_usd.is_finite());
        assert!(!report.baseline.cost_complete);
        assert_eq!(report.baseline.mean_cost_usd, None);
        assert_eq!(report.savings_fraction, None);
    }

    #[test]
    fn missing_pairs_usage_or_grading_never_justify_a_winner() {
        let baseline = sample(InferenceExperimentArm::Baseline, 1, Some(true), 100);
        let mut candidate = sample(InferenceExperimentArm::Candidate, 1, None, 1);
        let unpaired = table().compare_tasks(std::slice::from_ref(&baseline));
        assert!(!unpaired.paired);
        assert_eq!(unpaired.savings_fraction, None);
        candidate.accounting.attempts[0].usage = None;
        let incomplete = table().compare_tasks(&[baseline.clone(), candidate.clone()]);
        assert_eq!(incomplete.savings_fraction, None);
        assert_eq!(incomplete.quality_preserved, None);
        assert!(!incomplete.candidate.cost_complete);
        let duplicate = table().compare_tasks(&[baseline.clone(), baseline, candidate]);
        assert!(!duplicate.paired);
    }
}
