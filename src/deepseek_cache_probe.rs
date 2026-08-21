//! Opt-in paired measurement of DeepSeek automatic prefix-cache behavior.

use std::io::Write;
use std::num::{NonZeroU32, NonZeroUsize};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use anyhow::{Context, Result, bail};
use async_trait::async_trait;
use secrecy::ExposeSecret;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::agent::{AgentOutputMode, Message};
use crate::config::{DEFAULT_DEEPSEEK_BASE_URL, OpenAiEndpointKind, RaraConfig};
use crate::embedded::{EmbeddedRuntime, EmbeddedRuntimeOptions};
use crate::llm::{
    ContextBudget, LlmBackend, LlmResponse, LlmStreamEvent, LlmTurnMetadata,
    OpenAiCompatibleBackend, ProviderCacheProfile,
};
use crate::model_observation::{ModelRequestFingerprint, QueryReport};

pub const DEFAULT_DEEPSEEK_CACHE_PROBE_MODEL: &str = "deepseek-v4-flash";
const MAX_PAIRS: usize = 20;
const MAX_TURNS_PER_ARM: usize = 10;
const MAX_OUTPUT_TOKENS: u32 = 256;

/// One arm of the paired cache experiment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeepseekCacheProbeArm {
    /// Keep the first system-message marker stable across an arm's requests.
    StablePrefix,
    /// Change the first system-message marker before every request.
    CacheBusted,
}

impl DeepseekCacheProbeArm {
    fn label(self) -> &'static str {
        match self {
            Self::StablePrefix => "stable_prefix",
            Self::CacheBusted => "cache_busted",
        }
    }
}

/// Bounds and state isolation for one live DeepSeek cache probe.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeepseekCacheProbeOptions {
    pub model: String,
    pub pairs: NonZeroUsize,
    pub turns_per_arm: NonZeroUsize,
    pub max_output_tokens: NonZeroU32,
    /// Parent directory for isolated, per-arm RARA state.
    pub state_root: PathBuf,
}

impl Default for DeepseekCacheProbeOptions {
    fn default() -> Self {
        Self {
            model: DEFAULT_DEEPSEEK_CACHE_PROBE_MODEL.to_string(),
            pairs: NonZeroUsize::new(3).expect("three is non-zero"),
            turns_per_arm: NonZeroUsize::new(3).expect("three is non-zero"),
            max_output_tokens: NonZeroU32::new(64).expect("64 is non-zero"),
            state_root: std::env::temp_dir().join("rara-deepseek-cache-probe"),
        }
    }
}

impl DeepseekCacheProbeOptions {
    /// Number of logical main-model requests in a complete probe.
    ///
    /// Provider transport retries can add network attempts.
    pub fn planned_request_count(&self) -> usize {
        self.pairs
            .get()
            .saturating_mul(self.turns_per_arm.get())
            .saturating_mul(2)
    }

    /// Validate local experiment bounds without performing network requests.
    pub fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            bail!("DeepSeek cache probe model must not be empty");
        }
        if self.pairs.get() > MAX_PAIRS {
            bail!("DeepSeek cache probe supports at most {MAX_PAIRS} pairs");
        }
        if !(2..=MAX_TURNS_PER_ARM).contains(&self.turns_per_arm.get()) {
            bail!("DeepSeek cache probe turns_per_arm must be between 2 and {MAX_TURNS_PER_ARM}");
        }
        if self.max_output_tokens.get() > MAX_OUTPUT_TOKENS {
            bail!("DeepSeek cache probe max_output_tokens must not exceed {MAX_OUTPUT_TOKENS}");
        }
        Ok(())
    }
}

/// One scripted user turn and all main-model requests it produced.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepseekCacheProbeSample {
    pub pair_index: usize,
    pub arm_order: usize,
    pub arm: DeepseekCacheProbeArm,
    pub turn_index: usize,
    pub query_report: QueryReport,
}

/// Aggregated post-warmup metrics for one arm.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepseekCacheProbeArmSummary {
    pub model_turns: usize,
    pub measured_model_turns: usize,
    pub unmeasured_model_turns: usize,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_hit_tokens: u64,
    pub cache_miss_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate_basis_points: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub median_duration_ms: Option<u64>,
}

/// Comparison derived from the stable-prefix and cache-busted arms.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepseekCacheProbeSummary {
    pub warmup_turns_excluded_per_arm: usize,
    pub stable_prefix: DeepseekCacheProbeArmSummary,
    pub cache_busted: DeepseekCacheProbeArmSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cache_hit_rate_delta_basis_points: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inconclusive_reason: Option<String>,
}

/// Complete paired experiment report. Raw prompts and responses are excluded.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeepseekCacheProbeReport {
    pub run_id: String,
    pub model: String,
    pub pairs: usize,
    pub turns_per_arm: usize,
    pub planned_request_count: usize,
    pub samples: Vec<DeepseekCacheProbeSample>,
    pub summary: DeepseekCacheProbeSummary,
}

impl DeepseekCacheProbeReport {
    /// Write one run record, one record per sample, and one summary as JSONL.
    pub fn write_jsonl(&self, mut writer: impl Write) -> Result<()> {
        write_json_line(
            &mut writer,
            &json!({
                "record_type": "run",
                "run_id": self.run_id,
                "model": self.model,
                "pairs": self.pairs,
                "turns_per_arm": self.turns_per_arm,
                "planned_request_count": self.planned_request_count,
            }),
        )?;
        for sample in &self.samples {
            write_json_line(
                &mut writer,
                &json!({
                    "record_type": "sample",
                    "run_id": self.run_id,
                    "sample": sample,
                }),
            )?;
        }
        write_json_line(
            &mut writer,
            &json!({
                "record_type": "summary",
                "run_id": self.run_id,
                "summary": self.summary,
            }),
        )?;
        Ok(())
    }
}

/// Run an opt-in AB/BA experiment against DeepSeek's official chat-completions API.
///
/// The caller supplies credentials through `RaraConfig`. The function never
/// serializes the credential and always forces the official DeepSeek base URL,
/// disables model tools, limits output tokens, and creates isolated per-arm state.
pub async fn run_deepseek_cache_probe(
    config: &RaraConfig,
    workspace_root: impl AsRef<Path>,
    options: DeepseekCacheProbeOptions,
) -> Result<DeepseekCacheProbeReport> {
    options.validate()?;
    let api_key = config
        .api_key_secret()
        .context("DeepSeek cache probe requires an API key in RaraConfig")?;
    if api_key.expose_secret().trim().is_empty() {
        bail!("DeepSeek cache probe API key must not be empty");
    }
    let workspace_root = workspace_root.as_ref();
    if !workspace_root.is_dir() {
        bail!(
            "DeepSeek cache probe workspace does not exist: {}",
            workspace_root.display()
        );
    }

    let run_id = Uuid::new_v4().to_string();
    let run_state_root = options.state_root.join(&run_id);
    std::fs::create_dir_all(&run_state_root).with_context(|| {
        format!(
            "create DeepSeek cache probe state root {}",
            run_state_root.display()
        )
    })?;

    let mut probe_config = RaraConfig::default();
    probe_config.set_provider("deepseek");
    probe_config.set_api_key(api_key.expose_secret().to_string());
    probe_config.set_model(Some(options.model.clone()));
    probe_config.set_thinking(Some(false));

    let planned_request_count = options.planned_request_count();
    let pairs = options.pairs.get();
    let turns_per_arm = options.turns_per_arm.get();
    let mut samples = Vec::with_capacity(planned_request_count);
    for pair_index in 0..options.pairs.get() {
        let arm_order = if pair_index % 2 == 0 {
            [
                DeepseekCacheProbeArm::StablePrefix,
                DeepseekCacheProbeArm::CacheBusted,
            ]
        } else {
            [
                DeepseekCacheProbeArm::CacheBusted,
                DeepseekCacheProbeArm::StablePrefix,
            ]
        };
        for (order_index, arm) in arm_order.into_iter().enumerate() {
            let arm_samples = run_probe_arm(ProbeArmRun {
                config: &probe_config,
                workspace_root,
                options: &options,
                run_state_root: &run_state_root,
                pair_index,
                arm_order: order_index,
                arm,
            })
            .await
            .with_context(|| {
                format!(
                    "run DeepSeek cache probe pair {pair_index} arm {}",
                    arm.label()
                )
            })?;
            samples.extend(arm_samples);
        }
    }

    let summary = summarize_samples(&samples);
    Ok(DeepseekCacheProbeReport {
        run_id,
        model: options.model,
        pairs,
        turns_per_arm,
        planned_request_count,
        samples,
        summary,
    })
}

struct ProbeArmRun<'a> {
    config: &'a RaraConfig,
    workspace_root: &'a Path,
    options: &'a DeepseekCacheProbeOptions,
    run_state_root: &'a Path,
    pair_index: usize,
    arm_order: usize,
    arm: DeepseekCacheProbeArm,
}

async fn run_probe_arm(run: ProbeArmRun<'_>) -> Result<Vec<DeepseekCacheProbeSample>> {
    let state_root =
        run.run_state_root
            .join(format!("pair-{}-{}", run.pair_index, run.arm.label()));
    let mut runtime = EmbeddedRuntime::from_config_with_options(
        run.config,
        run.workspace_root,
        EmbeddedRuntimeOptions {
            state_root: Some(state_root),
            ..EmbeddedRuntimeOptions::default()
        },
    )
    .await?;

    let backend = OpenAiCompatibleBackend::new_with_endpoint_kind_and_reasoning(
        run.config.api_key_secret(),
        DEFAULT_DEEPSEEK_BASE_URL.to_string(),
        run.options.model.clone(),
        OpenAiEndpointKind::Deepseek,
        run.config.reasoning_effort.clone(),
        Some(false),
    )?
    .with_max_output_tokens(run.options.max_output_tokens);
    let backend = backend.with_deepseek_user_id(Uuid::new_v4().to_string());
    runtime.replace_llm_backend(Arc::new(CacheProbeBackend::new(Arc::new(backend), run.arm)));
    runtime.disable_tools();
    runtime.disable_extension_execution();
    runtime.set_max_turns(1);

    let mut samples = Vec::with_capacity(run.options.turns_per_arm.get());
    for turn_index in 0..run.options.turns_per_arm.get() {
        let prompt = scripted_prompt(turn_index);
        let query_report = runtime
            .query_with_report(prompt, AgentOutputMode::Silent, |_| {})
            .await?;
        samples.push(DeepseekCacheProbeSample {
            pair_index: run.pair_index,
            arm_order: run.arm_order,
            arm: run.arm,
            turn_index,
            query_report,
        });
    }
    Ok(samples)
}

fn scripted_prompt(turn_index: usize) -> String {
    format!(
        "Cache measurement turn {}. Reply with exactly: ack-{}",
        turn_index + 1,
        turn_index + 1
    )
}

fn summarize_samples(samples: &[DeepseekCacheProbeSample]) -> DeepseekCacheProbeSummary {
    let stable_prefix = summarize_arm(samples, DeepseekCacheProbeArm::StablePrefix);
    let cache_busted = summarize_arm(samples, DeepseekCacheProbeArm::CacheBusted);
    let cache_hit_rate_delta_basis_points = stable_prefix
        .cache_hit_rate_basis_points
        .zip(cache_busted.cache_hit_rate_basis_points)
        .map(|(stable, busted)| i32::from(stable) - i32::from(busted));
    let inconclusive_reason = if cache_hit_rate_delta_basis_points.is_none() {
        Some(
            "DeepSeek did not return usable cache accounting for both post-warmup arms."
                .to_string(),
        )
    } else {
        None
    };

    DeepseekCacheProbeSummary {
        warmup_turns_excluded_per_arm: 1,
        stable_prefix,
        cache_busted,
        cache_hit_rate_delta_basis_points,
        inconclusive_reason,
    }
}

fn summarize_arm(
    samples: &[DeepseekCacheProbeSample],
    arm: DeepseekCacheProbeArm,
) -> DeepseekCacheProbeArmSummary {
    let mut summary = DeepseekCacheProbeArmSummary::default();
    let mut durations = Vec::new();
    for sample in samples
        .iter()
        .filter(|sample| sample.arm == arm && sample.turn_index > 0)
    {
        for model_turn in &sample.query_report.model_turns {
            summary.model_turns += 1;
            durations.push(model_turn.duration_ms);
            let Some(usage) = model_turn.usage else {
                summary.unmeasured_model_turns += 1;
                continue;
            };
            summary.input_tokens = summary
                .input_tokens
                .saturating_add(u64::from(usage.input_tokens));
            summary.output_tokens = summary
                .output_tokens
                .saturating_add(u64::from(usage.output_tokens));
            let Some(cache) = usage.cache else {
                summary.unmeasured_model_turns += 1;
                continue;
            };
            summary.measured_model_turns += 1;
            summary.cache_hit_tokens = summary
                .cache_hit_tokens
                .saturating_add(u64::from(cache.hit_tokens));
            summary.cache_miss_tokens = summary
                .cache_miss_tokens
                .saturating_add(u64::from(cache.miss_tokens));
        }
    }
    let cache_total = summary
        .cache_hit_tokens
        .saturating_add(summary.cache_miss_tokens);
    if cache_total > 0 {
        let basis_points = summary
            .cache_hit_tokens
            .saturating_mul(10_000)
            .checked_div(cache_total)
            .unwrap_or_default();
        summary.cache_hit_rate_basis_points = Some(basis_points.min(u64::from(u16::MAX)) as u16);
    }
    durations.sort_unstable();
    summary.median_duration_ms = median(&durations);
    summary
}

fn median(values: &[u64]) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let middle = values.len() / 2;
    if values.len() % 2 == 1 {
        Some(values[middle])
    } else {
        Some(values[middle - 1].saturating_add(values[middle]) / 2)
    }
}

fn write_json_line(writer: &mut impl Write, value: &Value) -> Result<()> {
    serde_json::to_writer(&mut *writer, value)?;
    writer.write_all(b"\n")?;
    Ok(())
}

struct CacheProbeBackend {
    inner: Arc<dyn LlmBackend>,
    arm: DeepseekCacheProbeArm,
    next_request_index: AtomicUsize,
}

impl CacheProbeBackend {
    fn new(inner: Arc<dyn LlmBackend>, arm: DeepseekCacheProbeArm) -> Self {
        Self {
            inner,
            arm,
            next_request_index: AtomicUsize::new(0),
        }
    }

    fn peek_request_index(&self) -> usize {
        self.next_request_index.load(Ordering::SeqCst)
    }

    fn take_request_index(&self) -> usize {
        self.next_request_index.fetch_add(1, Ordering::SeqCst)
    }

    fn messages_for_request(&self, messages: &[Message], request_index: usize) -> Vec<Message> {
        let marker_index = match self.arm {
            DeepseekCacheProbeArm::StablePrefix => 0,
            DeepseekCacheProbeArm::CacheBusted => request_index,
        };
        let marker = format!("<rara_cache_probe request=\"{marker_index}\" />\n");
        let mut prepared = messages.to_vec();
        if let Some(system) = prepared
            .first_mut()
            .filter(|message| message.role == "system")
            && let Some(content) = system.content.as_str()
        {
            system.content = Value::String(format!("{marker}{content}"));
            return prepared;
        }
        prepared.insert(
            0,
            Message {
                role: "system".to_string(),
                content: Value::String(marker),
            },
        );
        prepared
    }
}

#[async_trait]
impl LlmBackend for CacheProbeBackend {
    fn model_label(&self) -> Option<String> {
        self.inner.model_label()
    }

    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        let messages = self.messages_for_request(messages, self.take_request_index());
        self.inner.ask(&messages, tools).await
    }

    async fn ask_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
    ) -> Result<LlmResponse> {
        let messages = self.messages_for_request(messages, self.take_request_index());
        self.inner
            .ask_with_context(&messages, tools, metadata)
            .await
    }

    async fn ask_streaming(
        &self,
        messages: &[Message],
        tools: &[Value],
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let messages = self.messages_for_request(messages, self.take_request_index());
        self.inner.ask_streaming(&messages, tools, on_event).await
    }

    async fn ask_streaming_with_context(
        &self,
        messages: &[Message],
        tools: &[Value],
        metadata: LlmTurnMetadata,
        on_event: &mut (dyn FnMut(LlmStreamEvent) + Send),
    ) -> Result<LlmResponse> {
        let messages = self.messages_for_request(messages, self.take_request_index());
        self.inner
            .ask_streaming_with_context(&messages, tools, metadata, on_event)
            .await
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        self.inner.summarize(messages, instruction).await
    }

    async fn classify(&self, instructions: &str, messages: &[Message]) -> Result<String> {
        self.inner.classify(instructions, messages).await
    }

    fn context_budget(&self, messages: &[Message], tools: &[Value]) -> Option<ContextBudget> {
        let messages = self.messages_for_request(messages, self.peek_request_index());
        self.inner.context_budget(&messages, tools)
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
        let messages = self.messages_for_request(messages, self.peek_request_index());
        self.inner
            .request_cache_fingerprint(&messages, tools, metadata)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use anyhow::Result;
    use serde_json::json;
    use sha2::{Digest, Sha256};

    use super::*;
    use crate::llm::{ContentBlock, TokenUsage};
    use crate::model_observation::{ModelCacheUsage, ModelTokenUsage, ModelTurnReport};

    struct FingerprintBackend {
        seen_system_messages: Mutex<Vec<String>>,
    }

    #[async_trait]
    impl LlmBackend for FingerprintBackend {
        async fn ask(&self, messages: &[Message], _tools: &[Value]) -> Result<LlmResponse> {
            self.seen_system_messages
                .lock()
                .expect("seen messages lock")
                .push(messages[0].content.as_str().unwrap_or_default().to_string());
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "ack".to_string(),
                }],
                stop_reason: Some("stop".to_string()),
                usage: Some(TokenUsage::default()),
            })
        }

        async fn summarize(&self, _messages: &[Message], _instruction: &str) -> Result<String> {
            Ok(String::new())
        }

        fn request_cache_fingerprint(
            &self,
            messages: &[Message],
            _tools: &[Value],
            _metadata: &LlmTurnMetadata,
        ) -> Option<ModelRequestFingerprint> {
            let content = messages[0].content.as_str().unwrap_or_default();
            let digest = Sha256::digest(content.as_bytes())
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            Some(ModelRequestFingerprint {
                version: 1,
                hash_scope: "test-scope".to_string(),
                request_sha256: digest.clone(),
                system_sha256: Some(digest.clone()),
                messages_sha256: digest.clone(),
                tools_sha256: digest.clone(),
                options_sha256: digest.clone(),
                message_sha256: vec![digest],
                tool_sha256: Vec::new(),
                message_count: 1,
                tool_count: 0,
            })
        }
    }

    fn message() -> Vec<Message> {
        vec![Message {
            role: "system".to_string(),
            content: Value::String("system".to_string()),
        }]
    }

    #[tokio::test]
    async fn stable_arm_preserves_marker_while_busted_arm_rotates_it() {
        for (arm, should_match) in [
            (DeepseekCacheProbeArm::StablePrefix, true),
            (DeepseekCacheProbeArm::CacheBusted, false),
        ] {
            let backend = CacheProbeBackend::new(
                Arc::new(FingerprintBackend {
                    seen_system_messages: Mutex::new(Vec::new()),
                }),
                arm,
            );
            let before = backend
                .request_cache_fingerprint(&message(), &[], &LlmTurnMetadata::default())
                .expect("first fingerprint");
            backend.ask(&message(), &[]).await.expect("first request");
            let after = backend
                .request_cache_fingerprint(&message(), &[], &LlmTurnMetadata::default())
                .expect("second fingerprint");

            assert_eq!(before.system_sha256 == after.system_sha256, should_match);
        }
    }

    #[test]
    fn summary_excludes_warmup_and_compares_cache_rates() {
        let sample = |arm, turn_index, hit_tokens, miss_tokens| DeepseekCacheProbeSample {
            pair_index: 0,
            arm_order: usize::from(arm == DeepseekCacheProbeArm::CacheBusted),
            arm,
            turn_index,
            query_report: QueryReport {
                model_turns: vec![ModelTurnReport {
                    model: "deepseek-v4-flash".to_string(),
                    duration_ms: 100,
                    finish_reason: Some("stop".to_string()),
                    usage: Some(ModelTokenUsage {
                        input_tokens: hit_tokens + miss_tokens,
                        output_tokens: 1,
                        cache: Some(ModelCacheUsage {
                            hit_tokens,
                            miss_tokens,
                        }),
                    }),
                    request_fingerprint: None,
                }],
            },
        };
        let samples = vec![
            sample(DeepseekCacheProbeArm::StablePrefix, 0, 0, 100),
            sample(DeepseekCacheProbeArm::StablePrefix, 1, 80, 20),
            sample(DeepseekCacheProbeArm::CacheBusted, 0, 0, 100),
            sample(DeepseekCacheProbeArm::CacheBusted, 1, 0, 100),
        ];

        let summary = summarize_samples(&samples);

        assert_eq!(
            summary.stable_prefix.cache_hit_rate_basis_points,
            Some(8_000)
        );
        assert_eq!(summary.cache_busted.cache_hit_rate_basis_points, Some(0));
        assert_eq!(summary.cache_hit_rate_delta_basis_points, Some(8_000));
        assert_eq!(summary.inconclusive_reason, None);
        assert_eq!(
            serde_json::to_value(summary).expect("serialize")["stable_prefix"]["cache_hit_tokens"],
            json!(80)
        );
    }

    #[test]
    fn options_reject_unbounded_live_runs() {
        let mut options = DeepseekCacheProbeOptions::default();
        assert!(options.validate().is_ok());

        options.pairs = NonZeroUsize::new(MAX_PAIRS + 1).expect("positive pairs");
        assert!(options.validate().is_err());

        options = DeepseekCacheProbeOptions::default();
        options.turns_per_arm = NonZeroUsize::new(1).expect("positive turns");
        assert!(options.validate().is_err());

        options = DeepseekCacheProbeOptions::default();
        options.max_output_tokens =
            NonZeroU32::new(MAX_OUTPUT_TOKENS + 1).expect("positive output tokens");
        assert!(options.validate().is_err());
    }
}
