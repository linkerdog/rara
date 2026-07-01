mod model;
mod parser;
mod prompt;

#[cfg(test)]
mod tests;

use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow};
use async_trait::async_trait;
use candle::{DType, Tensor};
use candle_transformers::generation::{LogitsProcessor, Sampling};
use serde_json::Value;

use self::model::{
    LocalModelSpec, LocalTextModel, build_hf_api, default_local_model_cache_dir as model_cache_dir,
    load_safetensors, preferred_dtype, select_device,
};
pub use self::model::{default_local_model_cache_dir, local_runtime_target};
use self::parser::parse_tool_aware_reply;
use self::prompt::{build_agent_prompt, render_messages, scenario_token_cap};
use crate::agent::Message;
use crate::config::RaraConfig;
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, TokenUsage, hashed_embedding};

pub type LocalProgressReporter = Arc<dyn Fn(String) + Send + Sync>;

pub struct LocalLlmBackend {
    runtime: Arc<Mutex<LocalRuntime>>,
    max_new_tokens: usize,
}

struct LocalRuntime {
    spec: LocalModelSpec,
    model: LocalTextModel,
    tokenizer: tokenizers::Tokenizer,
    device: candle::Device,
    eos_token_ids: Vec<u32>,
    context_window: usize,
}

struct GenerationResult {
    text: String,
    input_tokens: u32,
    output_tokens: u32,
}

impl LocalLlmBackend {
    pub fn from_config_with_progress(
        config: &RaraConfig,
        progress: Option<LocalProgressReporter>,
    ) -> Result<Self> {
        let spec = LocalModelSpec::from_config(config)?;
        let revision = config
            .revision
            .clone()
            .unwrap_or_else(|| "main".to_string());
        let cache_dir = model_cache_dir();
        report_progress(
            &progress,
            format!("Download setup · {} ({revision})", spec.alias()),
        );
        report_progress(&progress, format!("Cache · {}", cache_dir.display()));
        let api = build_hf_api(config, &cache_dir)?;
        let repo = api.repo(hf_hub::Repo::with_revision(
            spec.model_id().to_string(),
            hf_hub::RepoType::Model,
            revision,
        ));

        report_progress(
            &progress,
            format!("Manifest · resolving {}", spec.model_id()),
        );
        report_progress(&progress, "Artifact · tokenizer.json".to_string());
        let tokenizer_path = repo
            .get("tokenizer.json")
            .context("download tokenizer.json")?;
        report_progress(&progress, "Artifact · config.json".to_string());
        let config_path = repo.get("config.json").context("download config.json")?;
        report_progress(&progress, "Weights · downloading model weights".to_string());
        let weight_paths = load_safetensors(&repo)
            .or_else(|_| repo.get("model.safetensors").map(|p| vec![p]))
            .context("download model weights")?;
        report_progress(
            &progress,
            format!("Weights · {} file(s) ready", weight_paths.len()),
        );

        let raw_config: Value =
            serde_json::from_slice(&std::fs::read(&config_path).context("read config.json")?)
                .context("parse config.json")?;
        let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
            .map_err(|e| anyhow!("load tokenizer: {e}"))?;

        let device = select_device()?;
        let dtype = preferred_dtype(&device);
        report_progress(
            &progress,
            format!("Runtime · initializing on {:?} with {:?}", device, dtype),
        );
        let vb = unsafe {
            candle_nn::VarBuilder::from_mmaped_safetensors(&weight_paths, dtype, &device)?
        };
        let model = spec.build_model(&raw_config, vb)?;
        let eos_token_ids = spec.eos_token_ids(&tokenizer)?;
        let context_window = spec.context_window(&raw_config);
        report_progress(&progress, format!("Ready · {} loaded", spec.alias()));

        Ok(Self {
            runtime: Arc::new(Mutex::new(LocalRuntime {
                spec,
                model,
                tokenizer,
                device,
                eos_token_ids,
                context_window,
            })),
            max_new_tokens: 384,
        })
    }
}

fn report_progress(progress: &Option<LocalProgressReporter>, message: String) {
    if let Some(callback) = progress {
        callback(message);
    }
}

#[async_trait]
impl LlmBackend for LocalLlmBackend {
    async fn ask(&self, messages: &[Message], tools: &[Value]) -> Result<LlmResponse> {
        let runtime = Arc::clone(&self.runtime);
        let messages = messages.to_vec();
        let tools = tools.to_vec();
        let max_new_tokens = self.max_new_tokens;
        let result = tokio::task::spawn_blocking(move || {
            runtime
                .lock()
                .map_err(|_| anyhow!("local model runtime mutex poisoned"))?
                .generate(&messages, &tools, max_new_tokens)
        })
        .await
        .context("local model worker task join failed")??;
        let raw = result.text;

        let content = match parse_tool_aware_reply(&raw) {
            Ok(parsed) => {
                let mut blocks = Vec::new();
                if let Some(text) = parsed.text.filter(|t| !t.trim().is_empty()) {
                    blocks.push(ContentBlock::Text { text });
                }
                if parsed.kind.as_deref() == Some("tool") {
                    for (idx, call) in parsed.calls.unwrap_or_default().into_iter().enumerate() {
                        blocks.push(ContentBlock::ToolUse {
                            id: format!("local-tool-{}", idx + 1),
                            name: call.name,
                            input: call.input,
                        });
                    }
                }
                if blocks.is_empty() {
                    vec![ContentBlock::Text {
                        text: raw.trim().to_string(),
                    }]
                } else {
                    blocks
                }
            }
            Err(_) => vec![ContentBlock::Text {
                text: raw.trim().to_string(),
            }],
        };

        Ok(LlmResponse {
            stop_reason: Some("end_turn".to_string()),
            content,
            usage: Some(TokenUsage {
                input_tokens: result.input_tokens,
                output_tokens: result.output_tokens,
                ..TokenUsage::default()
            }),
        })
    }

    async fn embed(&self, text: &str) -> Result<Vec<f32>> {
        Ok(hashed_embedding(text, 256))
    }

    async fn summarize(&self, messages: &[Message], instruction: &str) -> Result<String> {
        let runtime = Arc::clone(&self.runtime);
        let messages = messages.to_vec();
        let instruction = instruction.to_string();
        tokio::task::spawn_blocking(move || {
            runtime
                .lock()
                .map_err(|_| anyhow!("local model runtime mutex poisoned"))?
                .summarize(&messages, &instruction)
        })
        .await
        .context("local model summary task join failed")?
    }
}

impl LocalRuntime {
    fn generate(
        &mut self,
        messages: &[Message],
        tools: &[Value],
        max_new_tokens: usize,
    ) -> Result<GenerationResult> {
        let base_prompt = build_agent_prompt(messages, tools);
        let prompt = self.spec.format_prompt(&base_prompt);
        self.model.clear_kv_cache();

        let prompt_tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| anyhow!("tokenize prompt: {e}"))?
            .get_ids()
            .to_vec();
        if prompt_tokens.is_empty() {
            return Ok(GenerationResult {
                text: String::new(),
                input_tokens: 0,
                output_tokens: 0,
            });
        }
        let max_new_tokens =
            self.suggest_max_new_tokens(messages, tools, prompt_tokens.len(), max_new_tokens);

        let mut tokens = prompt_tokens.clone();
        let mut generated = Vec::new();
        let mut processor = LogitsProcessor::from_sampling(rand::random(), Sampling::ArgMax);

        for index in 0..max_new_tokens {
            let context_size = if index == 0 { tokens.len() } else { 1 };
            let start_pos = tokens.len().saturating_sub(context_size);
            let input = Tensor::new(&tokens[start_pos..], &self.device)?.unsqueeze(0)?;
            let logits = self
                .model
                .forward(&input, start_pos)?
                .squeeze(0)?
                .squeeze(0)?
                .to_dtype(DType::F32)?;
            let next_token = processor.sample(&logits)?;
            if self.eos_token_ids.contains(&next_token) {
                break;
            }
            tokens.push(next_token);
            generated.push(next_token);
        }

        let text = self
            .tokenizer
            .decode(&generated, true)
            .map_err(|e| anyhow!("decode output: {e}"))?;
        Ok(GenerationResult {
            text,
            input_tokens: prompt_tokens.len() as u32,
            output_tokens: generated.len() as u32,
        })
    }

    fn suggest_max_new_tokens(
        &self,
        messages: &[Message],
        tools: &[Value],
        prompt_tokens: usize,
        configured_cap: usize,
    ) -> usize {
        let scenario_cap = scenario_token_cap(messages, tools);
        let safety_buffer = 256usize;
        let safe_budget = self
            .context_window
            .saturating_sub(prompt_tokens.saturating_add(safety_buffer))
            .max(48);
        safe_budget.min(configured_cap).min(scenario_cap).max(48)
    }

    fn summarize(&mut self, messages: &[Message], instruction: &str) -> Result<String> {
        let prompt = format!("{}\n\n{}", instruction, render_messages(messages));
        let prompt = self.spec.format_prompt(&prompt);
        self.model.clear_kv_cache();

        let prompt_tokens = self
            .tokenizer
            .encode(prompt, true)
            .map_err(|e| anyhow!("tokenize prompt: {e}"))?
            .get_ids()
            .to_vec();
        if prompt_tokens.is_empty() {
            return Ok(String::new());
        }

        let mut tokens = prompt_tokens.clone();
        let mut generated = Vec::new();
        let mut processor = LogitsProcessor::from_sampling(rand::random(), Sampling::ArgMax);

        for index in 0..256 {
            let context_size = if index == 0 { tokens.len() } else { 1 };
            let start_pos = tokens.len().saturating_sub(context_size);
            let input = Tensor::new(&tokens[start_pos..], &self.device)?.unsqueeze(0)?;
            let logits = self
                .model
                .forward(&input, start_pos)?
                .squeeze(0)?
                .squeeze(0)?
                .to_dtype(DType::F32)?;
            let next_token = processor.sample(&logits)?;
            if self.eos_token_ids.contains(&next_token) {
                break;
            }
            tokens.push(next_token);
            generated.push(next_token);
        }

        self.tokenizer
            .decode(&generated, true)
            .map_err(|e| anyhow!("decode output: {e}"))
            .map(|s| s.trim().to_string())
    }
}
