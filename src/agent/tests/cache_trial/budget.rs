use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Result, bail, ensure};
use rara_observability::{InferencePriceTable, InferenceSnapshot, InferenceTask};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::llm::MAX_CHAT_ATTEMPTS_PER_CALL as MAX_ATTEMPTS_PER_CALL;
use crate::llm::{
    ContextBudget, LlmBackend, LlmResponse, LlmStreamEvent, LlmTurnMetadata, Message,
    ProviderCacheProfile, SummaryPrefix,
};
use crate::model_observation::ModelRequestFingerprint;
const MAX_INPUT_TOKENS: u64 = 1_048_576;
const CALL_DEADLINE: Duration = Duration::from_secs(90);

#[derive(Clone, Copy, Deserialize, Serialize)]
pub(super) struct TariffWindow {
    pub valid_from_unix_ms: u64,
    pub valid_until_unix_ms: u64,
}

impl TariffWindow {
    // A logical call must fit wholly within the supplied tariff window.
    pub(super) fn check_at(&self, now: SystemTime) -> Result<()> {
        let now = now.duration_since(UNIX_EPOCH)?.as_millis();
        ensure!(
            u128::from(self.valid_from_unix_ms) <= now
                && now + CALL_DEADLINE.as_millis() < u128::from(self.valid_until_unix_ms),
            "the next model call does not fit within the supplied tariff window"
        );
        Ok(())
    }
}

pub(super) struct Budget {
    limit: u64,
    committed: u64,
    reservation: u64,
    unsettled: bool,
    validity: Option<TariffWindow>,
}

impl Budget {
    pub(super) fn new(limit_usd: f64, prices: &InferencePriceTable, output: u32) -> Result<Self> {
        ensure!(
            limit_usd.is_finite() && limit_usd > 0.0 && limit_usd <= 100.0,
            "cost ceiling must be in (0, 100] USD"
        );
        ensure!(
            !prices.revision.trim().is_empty() && !prices.prices.is_empty(),
            "a dated price table is required"
        );
        let mut max_cost = 0.0_f64;
        for price in &prices.prices {
            let rates = [
                price.input,
                price.output,
                price.cache_read,
                price.cache_write,
                price.cache_write_5m,
                price.cache_write_1h,
            ];
            ensure!(
                rates.iter().all(|rate| rate.is_finite() && *rate >= 0.0),
                "invalid tariff"
            );
            let input_rate = [
                price.input,
                price.cache_read,
                price.cache_write,
                price.cache_write_5m,
                price.cache_write_1h,
            ]
            .into_iter()
            .fold(0.0_f64, f64::max);
            max_cost = max_cost
                .max(MAX_INPUT_TOKENS as f64 * input_rate + f64::from(output) * price.output);
        }
        let reservation = max_cost * MAX_ATTEMPTS_PER_CALL as f64;
        ensure!(
            reservation.is_finite() && reservation > 0.0 && reservation <= 100_000_000.0,
            "unsupported maximum request charge"
        );
        Ok(Self {
            limit: (limit_usd * 1_000_000.0).floor() as u64,
            committed: 0,
            reservation: reservation.ceil() as u64,
            unsettled: false,
            validity: None,
        })
    }

    pub(super) fn reserved_call_usd(&self) -> f64 {
        self.reservation as f64 / 1_000_000.0
    }

    pub(super) fn with_tariff_window(mut self, validity: TariffWindow) -> Result<Self> {
        validity.check_at(SystemTime::now())?;
        self.validity = Some(validity);
        Ok(self)
    }
}

pub(super) struct CappedBackend {
    pub inner: Arc<dyn LlmBackend>,
    pub task: InferenceTask,
    pub prices: InferencePriceTable,
    pub budget: Arc<Mutex<Budget>>,
}

impl CappedBackend {
    async fn guarded<T>(&self, future: impl Future<Output = Result<T>>) -> Result<T> {
        let reservation = {
            let mut budget = self.budget.lock().expect("trial budget");
            ensure!(!budget.unsettled, "trial has an unsettled model call");
            if let Some(validity) = budget.validity {
                validity.check_at(SystemTime::now())?;
            }
            let next = budget
                .committed
                .checked_add(budget.reservation)
                .filter(|next| *next <= budget.limit);
            let Some(next) = next else {
                bail!("trial cost ceiling cannot cover the next call's maximum charge");
            };
            budget.committed = next;
            budget.unsettled = true;
            budget.reservation
        };
        // If this future is dropped, the full reservation remains committed.
        let first_attempt = self.task.snapshot().attempts.len();
        let result = match tokio::time::timeout(CALL_DEADLINE, future).await {
            Ok(result) => result,
            Err(_) => Err(anyhow::anyhow!("trial model-call deadline exceeded")),
        };
        let snapshot = self.task.snapshot();
        let attempts = snapshot.attempts[first_attempt..].to_vec();
        let count = attempts.len();
        let cost = self.prices.cost(&InferenceSnapshot {
            attempts,
            poisoned: snapshot.poisoned,
            ..Default::default()
        });
        if count > 0 && count <= MAX_ATTEMPTS_PER_CALL && cost.complete {
            let charge = (cost.known_cost_usd * 1_000_000.0).ceil() as u64;
            ensure!(
                charge <= reservation,
                "provider charge exceeded the verified request bound"
            );
            let mut budget = self.budget.lock().expect("trial budget");
            budget.committed -= reservation - charge;
            budget.unsettled = false;
        }
        // Unknown retries keep their maximum reservation and stop the trial.
        ensure!(
            count > 0 && count <= MAX_ATTEMPTS_PER_CALL && cost.complete,
            "trial has incomplete physical-request accounting"
        );
        result
    }
}

#[async_trait::async_trait]
impl LlmBackend for CappedBackend {
    async fn ask(&self, _: &[Message], _: &[Value]) -> Result<LlmResponse> {
        bail!("trial requests require accounting metadata")
    }
    async fn summarize(&self, _: &[Message], _: &str) -> Result<String> {
        bail!("trial summaries require accounting metadata")
    }
    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        events: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        self.guarded(
            self.inner
                .ask_streaming_with_context(messages, tools, metadata, events),
        )
        .await
    }
    async fn summarize_with_context(
        &self,
        messages: &[Message],
        instruction: &str,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.guarded(
            self.inner
                .summarize_with_context(messages, instruction, metadata),
        )
        .await
    }
    async fn summarize_with_prefix(
        &self,
        messages: &[Message],
        instruction: &str,
        prefix: &SummaryPrefix,
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.guarded(
            self.inner
                .summarize_with_prefix(messages, instruction, prefix, metadata),
        )
        .await
    }
    async fn classify_with_context(
        &self,
        instruction: &str,
        messages: &[Message],
        metadata: LlmTurnMetadata,
    ) -> Result<String> {
        self.guarded(
            self.inner
                .classify_with_context(instruction, messages, metadata),
        )
        .await
    }
    fn model_label(&self) -> Option<String> {
        self.inner.model_label()
    }
    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        self.inner.context_budget(messages, tools)
    }
    fn cache_profile(&self) -> ProviderCacheProfile {
        self.inner.cache_profile()
    }
    fn request_cache_fingerprint(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: &LlmTurnMetadata,
    ) -> Option<ModelRequestFingerprint> {
        self.inner
            .request_cache_fingerprint(messages, tools, metadata)
    }
}
