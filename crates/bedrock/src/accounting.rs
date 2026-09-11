use std::sync::Mutex;

use aws_sdk_bedrockruntime::operation::converse::ConverseOutput;
use aws_sdk_bedrockruntime::types::TokenUsage;
use aws_smithy_runtime_api::box_error::BoxError;
use aws_smithy_runtime_api::client::interceptors::Intercept;
use aws_smithy_runtime_api::client::interceptors::context::{
    BeforeTransmitInterceptorContextRef, FinalizerInterceptorContextRef,
};
use aws_smithy_runtime_api::client::runtime_components::RuntimeComponents;
use aws_smithy_types::config_bag::ConfigBag;
use rara_observability::{InferenceAttempt, InferenceCallContext, InferenceTokenUsage};

/// One interceptor per Converse operation, including SDK-owned retries.
#[derive(Debug)]
pub(crate) struct AttemptAccounting {
    context: InferenceCallContext,
    model: String,
    provider: String,
    current: Mutex<Option<InferenceAttempt>>,
}

impl AttemptAccounting {
    pub(crate) fn new(context: InferenceCallContext, model: String, region: &str) -> Self {
        Self {
            context,
            model,
            provider: format!("Amazon Bedrock:{region}"),
            current: Mutex::new(None),
        }
    }
}

impl Intercept for AttemptAccounting {
    fn name(&self) -> &'static str {
        "TaskInferenceAccounting"
    }

    fn read_before_transmit(
        &self,
        _context: &BeforeTransmitInterceptorContextRef<'_>,
        _runtime: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let mut current = self
            .current
            .lock()
            .map_err(|_| "inference attempt lock poisoned")?;
        *current = Some(self.context.start_attempt(&self.provider, &self.model));
        Ok(())
    }

    fn read_after_attempt(
        &self,
        context: &FinalizerInterceptorContextRef<'_>,
        _runtime: &RuntimeComponents,
        _cfg: &mut ConfigBag,
    ) -> Result<(), BoxError> {
        let Some(attempt) = self
            .current
            .lock()
            .map_err(|_| "inference attempt lock poisoned")?
            .take()
        else {
            return Ok(());
        };
        if let Some(Ok(output)) = context.output_or_error()
            && let Some(output) = output.downcast_ref::<ConverseOutput>()
            && let Some(usage) = output.usage.as_ref().and_then(normalize_usage)
        {
            attempt.record_final_usage(usage);
        }
        let result = if matches!(context.output_or_error(), Some(Ok(_))) {
            Ok(())
        } else {
            Err(())
        };
        attempt.finish(&result);
        Ok(())
    }
}

fn normalize_usage(usage: &TokenUsage) -> Option<InferenceTokenUsage> {
    let ordinary = u64::try_from(usage.input_tokens).ok()?;
    let read = usage
        .cache_read_input_tokens
        .and_then(|value| u64::try_from(value).ok());
    let write = usage
        .cache_write_input_tokens
        .and_then(|value| u64::try_from(value).ok());
    let mut short = 0_u64;
    let mut long = 0_u64;
    for detail in usage.cache_details() {
        let tokens = u64::try_from(detail.input_tokens).ok()?;
        match detail.ttl.as_str() {
            "5m" => short = short.checked_add(tokens)?,
            "1h" => long = long.checked_add(tokens)?,
            // Future TTLs remain generic writes and require their own tariff.
            _ => {}
        }
    }
    Some(InferenceTokenUsage {
        input_tokens: ordinary
            .checked_add(read.unwrap_or(0))?
            .checked_add(write.unwrap_or(0))?,
        output_tokens: u64::try_from(usage.output_tokens).ok()?,
        cache_read_tokens: read,
        cache_write_tokens: write.and_then(|write| write.checked_sub(short.checked_add(long)?)),
        cache_write_5m_tokens: Some(short),
        cache_write_1h_tokens: Some(long),
    })
}

#[cfg(test)]
mod tests {
    use aws_sdk_bedrockruntime::types::{CacheDetail, CacheTtl};

    use super::*;

    #[test]
    fn converse_usage_keeps_cache_categories_disjoint() {
        let usage = TokenUsage::builder()
            .input_tokens(200)
            .output_tokens(100)
            .total_tokens(1100)
            .cache_read_input_tokens(600)
            .cache_write_input_tokens(200)
            .cache_details(
                CacheDetail::builder()
                    .ttl(CacheTtl::FiveMinutes)
                    .input_tokens(75)
                    .build()
                    .unwrap(),
            )
            .cache_details(
                CacheDetail::builder()
                    .ttl(CacheTtl::OneHour)
                    .input_tokens(125)
                    .build()
                    .unwrap(),
            )
            .build()
            .unwrap();
        let parsed = normalize_usage(&usage).unwrap();
        assert_eq!(parsed.input_tokens, 1000);
        assert_eq!(parsed.cache_write_tokens, Some(0));
        assert_eq!(parsed.cache_write_5m_tokens, Some(75));
        assert_eq!(parsed.cache_write_1h_tokens, Some(125));
        let absent = TokenUsage::builder()
            .input_tokens(10)
            .output_tokens(1)
            .total_tokens(11)
            .build()
            .unwrap();
        assert_eq!(normalize_usage(&absent).unwrap().cache_read_tokens, None);
    }
}
