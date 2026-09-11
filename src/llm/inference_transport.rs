use anyhow::{Result, anyhow};
use backon::{ExponentialBuilder, Retryable};
use rara_observability::InferenceAttempt;
use serde_json::Value;

use super::shared::{LlmTurnMetadata, is_retryable_http_error};

pub(super) const MAX_SEND_RETRIES: usize = 3;

/// Keep an attempt open through response decoding, not just HTTP headers.
pub(super) async fn send_json(
    request: reqwest::RequestBuilder,
    metadata: &LlmTurnMetadata,
    provider: &str,
    model: &str,
) -> Result<(reqwest::Response, Option<InferenceAttempt>)> {
    (|| async {
        metadata.ensure_not_cancelled()?;
        let request = request
            .try_clone()
            .ok_or_else(|| anyhow!("model request is not cloneable"))?;
        let attempt = metadata.start_attempt(provider, model);
        let result = request.send().await.map_err(anyhow::Error::from);
        match result {
            Ok(response)
                if response.status() == reqwest::StatusCode::TOO_MANY_REQUESTS
                    || response.status().is_server_error() =>
            {
                let error = response
                    .error_for_status_ref()
                    .expect_err("retryable status must be an HTTP error");
                if attempt.is_some() {
                    match response.json::<Value>().await {
                        Ok(body) => record_final_usage(&attempt, body.get("usage")),
                        Err(_) => log::warn!(
                            "Could not decode usage from a retryable model response; its bill remains incomplete"
                        ),
                    }
                }
                let result = Err(anyhow::Error::from(error));
                finish_attempt(attempt, &result);
                result
            }
            Ok(response) => Ok((response, attempt)),
            Err(error) => {
                let result = Err(error);
                finish_attempt(attempt, &result);
                result
            }
        }
    })
    .retry(
        ExponentialBuilder::default()
            .with_max_times(MAX_SEND_RETRIES)
            .with_jitter(),
    )
    .when(is_retryable_http_error)
    .await
}

pub(super) fn record_usage(attempt: &Option<InferenceAttempt>, usage: Option<&Value>) {
    if let Some(attempt) = attempt
        && let Some(usage) =
            usage.and_then(super::openai_compatible::inference_usage::parse_inference_usage)
    {
        attempt.record_usage(usage);
    }
}

pub(super) fn record_final_usage(attempt: &Option<InferenceAttempt>, usage: Option<&Value>) {
    if let Some(attempt) = attempt
        && let Some(usage) =
            usage.and_then(super::openai_compatible::inference_usage::parse_inference_usage)
    {
        attempt.record_final_usage(usage);
    }
}

pub(super) fn finish_attempt<T>(attempt: Option<InferenceAttempt>, result: &Result<T>) {
    if let Some(attempt) = attempt {
        attempt.finish(result);
    }
}
