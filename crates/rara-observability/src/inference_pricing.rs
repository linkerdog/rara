use serde::{Deserialize, Serialize};

use crate::{InferenceSnapshot, InferenceStatus, InferenceTokenUsage};

/// USD per million tokens, supplied for an exact provider/model identity.
/// Generic writes exclude the separately reported 5-minute and 1-hour writes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferencePrice {
    pub provider: String,
    pub model: String,
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
    pub cache_write: f64,
    pub cache_write_5m: f64,
    pub cache_write_1h: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferencePriceTable {
    pub revision: String,
    pub prices: Vec<InferencePrice>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct InferenceCostReport {
    pub price_revision: String,
    /// Includes known charges from failed attempts; never an estimate of unknown charges.
    pub known_cost_usd: f64,
    pub unpriced_attempts: usize,
    pub unobserved_calls: usize,
    pub complete: bool,
}

impl InferencePriceTable {
    pub fn cost(&self, snapshot: &InferenceSnapshot) -> InferenceCostReport {
        let mut report = InferenceCostReport {
            price_revision: self.revision.clone(),
            known_cost_usd: 0.0,
            unpriced_attempts: 0,
            unobserved_calls: snapshot
                .calls
                .iter()
                .filter(|call| !call.attempts_observed)
                .count(),
            complete: false,
        };
        for attempt in &snapshot.attempts {
            let mut prices = self
                .prices
                .iter()
                .filter(|price| price.provider == attempt.provider && price.model == attempt.model);
            let cost = prices.next().and_then(|price| {
                // Ambiguous tariffs are not resolved by iteration order.
                if prices.next().is_some() || self.revision.trim().is_empty() {
                    return None;
                }
                attempt.usage.as_ref().and_then(|usage| price.cost(usage))
            });
            match cost {
                Some(cost) => {
                    let total = report.known_cost_usd + cost.known_usd;
                    if total.is_finite() {
                        report.known_cost_usd = total;
                    }
                    if !total.is_finite()
                        || !cost.complete
                        || !attempt.usage_complete
                        || attempt.status == InferenceStatus::Running
                    {
                        report.unpriced_attempts += 1;
                    }
                }
                None => report.unpriced_attempts += 1,
            }
        }
        report.complete = snapshot.is_terminal()
            && !snapshot.poisoned
            && report.unpriced_attempts == 0
            && report.unobserved_calls == 0;
        report
    }
}

struct PricedUsage {
    known_usd: f64,
    complete: bool,
}

impl InferencePrice {
    fn cost(&self, usage: &InferenceTokenUsage) -> Option<PricedUsage> {
        let read = usage.cache_read_tokens;
        let write = usage.cache_write_tokens;
        let short = usage.cache_write_5m_tokens;
        let long = usage.cache_write_1h_tokens;
        let complete = [read, write, short, long].iter().all(Option::is_some);
        let cached = read
            .unwrap_or(0)
            .checked_add(write.unwrap_or(0))?
            .checked_add(short.unwrap_or(0))?
            .checked_add(long.unwrap_or(0))?;
        // Missing categories may account for the remaining input. Price only
        // independently known charges until the full breakdown is available.
        let ordinary = usage.input_tokens.checked_sub(cached)?;
        let categories = [
            (complete.then_some(ordinary), self.input),
            (Some(usage.output_tokens), self.output),
            (read, self.cache_read),
            (write, self.cache_write),
            (short, self.cache_write_5m),
            (long, self.cache_write_1h),
        ];
        let mut cost = 0.0;
        for (tokens, rate) in categories {
            if !rate.is_finite() || rate < 0.0 {
                return None;
            }
            if let Some(tokens) = tokens {
                cost += (tokens as f64 / 1_000_000.0) * rate;
            }
        }
        cost.is_finite().then_some(PricedUsage {
            known_usd: cost,
            complete,
        })
    }
}
