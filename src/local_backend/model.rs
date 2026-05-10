use std::path::PathBuf;

use anyhow::{Context, Result, anyhow};
use candle::{DType, Device};
use candle_nn::VarBuilder;
use candle_transformers::models::gemma4::{
    Model as Gemma4Model,
    config::{Gemma4Config, Gemma4TextConfig},
    text::TextModel as Gemma4TextModel,
};
use candle_transformers::models::qwen3::{Config as Qwen3Config, ModelForCausalLM as Qwen3Model};
use hf_hub::api::sync::{ApiBuilder, ApiRepo};
use serde_json::Value;
use tokenizers::Tokenizer;

use crate::config::RaraConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum LocalModelSpec {
    Gemma4E2B,
    Gemma4E4B,
    Qwen3_8B,
}

pub(super) enum LocalTextModel {
    Gemma4(Gemma4TextModel),
    Gemma4Multimodal(Gemma4Model),
    Qwen3(Qwen3Model),
}

impl LocalTextModel {
    pub(super) fn forward(
        &mut self,
        input: &candle::Tensor,
        offset: usize,
    ) -> candle::Result<candle::Tensor> {
        match self {
            Self::Gemma4(model) => model.forward(input, offset),
            Self::Gemma4Multimodal(model) => model.forward(input, offset),
            Self::Qwen3(model) => model.forward(input, offset),
        }
    }

    pub(super) fn clear_kv_cache(&mut self) {
        match self {
            Self::Gemma4(model) => model.clear_kv_cache(),
            Self::Gemma4Multimodal(model) => model.clear_kv_cache(),
            Self::Qwen3(model) => model.clear_kv_cache(),
        }
    }
}

impl LocalModelSpec {
    pub(super) fn from_config(config: &RaraConfig) -> Result<Self> {
        let provider = config.provider.trim();
        let model = config.model.as_deref().unwrap_or_default().trim();

        if provider == "qwen3" || provider == "qwn3" {
            return Ok(Self::Qwen3_8B);
        }
        if provider == "gemma4" {
            if model.eq_ignore_ascii_case("gemma4-e2b") || model.eq_ignore_ascii_case("gemma-4-e2b")
            {
                return Ok(Self::Gemma4E2B);
            }
            return Ok(Self::Gemma4E4B);
        }
        if provider == "local" || provider == "local-candle" {
            return Self::from_alias_or_model_id(model);
        }

        Self::from_alias_or_model_id(model)
    }

    pub(super) fn from_alias_or_model_id(value: &str) -> Result<Self> {
        let normalized = value.trim().to_ascii_lowercase();
        match normalized.as_str() {
            "" | "gemma4-e4b" | "gemma-4-e4b" | "google/gemma-4-e4b-it" => Ok(Self::Gemma4E4B),
            "gemma4-e2b" | "gemma-4-e2b" | "google/gemma-4-e2b-it" => Ok(Self::Gemma4E2B),
            "qwen3-8b" | "qwen-3-8b" | "qwen/qwen3-8b" | "qwn3-8b" | "qwn3 8b" => {
                Ok(Self::Qwen3_8B)
            }
            _ => Err(anyhow!(
                "unsupported local model '{value}', expected one of: gemma4-e4b, gemma4-e2b, qwen3-8b, qwn3-8b"
            )),
        }
    }

    pub(super) fn alias(self) -> &'static str {
        match self {
            Self::Gemma4E2B => "gemma4-e2b",
            Self::Gemma4E4B => "gemma4-e4b",
            Self::Qwen3_8B => "qwen3-8b",
        }
    }

    pub(super) fn model_id(self) -> &'static str {
        match self {
            Self::Gemma4E2B => "google/gemma-4-E2B-it",
            Self::Gemma4E4B => "google/gemma-4-E4B-it",
            Self::Qwen3_8B => "Qwen/Qwen3-8B",
        }
    }

    pub(super) fn context_window(self, raw_config: &Value) -> usize {
        extract_context_window(raw_config).unwrap_or(match self {
            Self::Gemma4E2B | Self::Gemma4E4B => 8192,
            Self::Qwen3_8B => 32768,
        })
    }

    pub(super) fn format_prompt(self, prompt: &str) -> String {
        match self {
            Self::Qwen3_8B => {
                format!("<|im_start|>user\n{prompt} /no_think<|im_end|>\n<|im_start|>assistant\n")
            }
            Self::Gemma4E2B | Self::Gemma4E4B => prompt.to_string(),
        }
    }

    pub(super) fn eos_token_ids(self, tokenizer: &Tokenizer) -> Result<Vec<u32>> {
        let vocab = tokenizer.get_vocab(true);
        let ids = match self {
            Self::Gemma4E2B | Self::Gemma4E4B => vec!["</s>", "<eos>"],
            Self::Qwen3_8B => vec!["<|endoftext|>", "<|im_end|>"],
        };
        let resolved: Vec<u32> = ids
            .into_iter()
            .filter_map(|token| vocab.get(token).copied())
            .collect();
        if resolved.is_empty() {
            return Err(anyhow!(
                "model '{}' tokenizer does not expose a known EOS token",
                self.alias()
            ));
        }
        Ok(resolved)
    }

    pub(super) fn build_model(self, raw_config: &Value, vb: VarBuilder) -> Result<LocalTextModel> {
        match self {
            Self::Gemma4E2B | Self::Gemma4E4B => build_gemma4_model(raw_config, vb),
            Self::Qwen3_8B => {
                let config: Qwen3Config =
                    serde_json::from_value(raw_config.clone()).context("parse Qwen3Config")?;
                Ok(LocalTextModel::Qwen3(
                    Qwen3Model::new(&config, vb).context("build Qwen3 model")?,
                ))
            }
        }
    }
}

fn is_multimodal_gemma4_checkpoint(raw_config: &Value) -> bool {
    raw_config
        .get("architectures")
        .and_then(Value::as_array)
        .map(|architectures| {
            architectures.iter().any(|value| {
                value
                    .as_str()
                    .map(|name| name == "Gemma4ForConditionalGeneration")
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
        || raw_config.get("vision_config").is_some()
        || raw_config.get("audio_config").is_some()
}

fn build_gemma4_model(raw_config: &Value, vb: VarBuilder) -> Result<LocalTextModel> {
    let is_multimodal_layout = is_multimodal_gemma4_checkpoint(raw_config);
    if is_multimodal_layout {
        let config: Gemma4Config =
            serde_json::from_value(raw_config.clone()).context("parse Gemma4Config")?;
        match Gemma4Model::new(&config, vb.clone()) {
            Ok(model) => return Ok(LocalTextModel::Gemma4Multimodal(model)),
            Err(err) if should_fallback_to_text_only(&err) => {}
            Err(err) => return Err(err).context("build Gemma4 multimodal model"),
        }
    }

    let mut text_config: Gemma4TextConfig = if let Some(text_cfg) = raw_config.get("text_config") {
        serde_json::from_value(text_cfg.clone()).context("parse text_config")?
    } else {
        serde_json::from_value(raw_config.clone()).context("parse Gemma4TextConfig")?
    };
    text_config.use_flash_attn = false;
    let text_vb = if is_multimodal_layout {
        vb.rename_f(remap_multimodal_gemma4_text_tensor)
    } else {
        vb
    };
    Ok(LocalTextModel::Gemma4(
        Gemma4TextModel::new(&text_config, text_vb).context("build Gemma4 text model")?,
    ))
}

fn should_fallback_to_text_only(err: &candle::Error) -> bool {
    let message = err.to_string();
    message.contains("cannot find tensor model.vision_tower")
        || message.contains("cannot find tensor model.audio_tower")
        || message.contains("cannot find tensor model.vision_encoder")
        || message.contains("cannot find tensor model.audio_encoder")
}

fn remap_multimodal_gemma4_text_tensor(name: &str) -> String {
    if let Some(suffix) = name.strip_prefix("model.") {
        format!("model.language_model.{suffix}")
    } else if let Some(suffix) = name.strip_prefix("lm_head.") {
        format!("model.language_model.lm_head.{suffix}")
    } else {
        name.to_string()
    }
}

pub(super) fn build_hf_api(
    config: &RaraConfig,
    cache_dir: &PathBuf,
) -> Result<hf_hub::api::sync::Api> {
    let mut builder = ApiBuilder::new()
        .with_cache_dir(cache_dir.clone())
        .with_progress(true)
        .with_retries(3);
    if let Some(token) = config
        .api_key()
        .map(str::to_owned)
        .or_else(|| std::env::var("HF_TOKEN").ok())
    {
        builder = builder.with_token(Some(token));
    }
    builder.build().context("build Hugging Face API client")
}

pub fn default_local_model_cache_dir() -> PathBuf {
    if let Ok(path) = std::env::var("RARA_MODEL_CACHE") {
        return PathBuf::from(path);
    }
    let xdg_cache = std::env::var("XDG_CACHE_HOME")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{}/.cache", h)));
    let mut base = xdg_cache
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(".rara"));
    base.push("rara");
    base.push("huggingface");
    base
}

pub fn local_runtime_target() -> Result<(String, String)> {
    let device = select_device()?;
    let dtype = preferred_dtype(&device);
    Ok((
        device_label(&device).to_string(),
        dtype_label(dtype).to_string(),
    ))
}

pub(super) fn load_safetensors(repo: &ApiRepo) -> Result<Vec<PathBuf>> {
    let index_path = repo
        .get("model.safetensors.index.json")
        .context("download model.safetensors.index.json")?;
    let reader = std::fs::File::open(&index_path).context("open safetensors index")?;
    let json: Value = serde_json::from_reader(reader).context("parse safetensors index")?;
    let weight_map = json
        .get("weight_map")
        .and_then(Value::as_object)
        .ok_or_else(|| anyhow!("invalid safetensors index: missing weight_map"))?;

    let mut files = std::collections::BTreeSet::new();
    for value in weight_map.values() {
        if let Some(file) = value.as_str() {
            files.insert(
                repo.get(file)
                    .with_context(|| format!("download weight shard {file}"))?,
            );
        }
    }
    if files.is_empty() {
        return Err(anyhow!("no weight shards found in safetensors index"));
    }
    Ok(files.into_iter().collect())
}

pub(super) fn extract_context_window(raw_config: &Value) -> Option<usize> {
    [
        raw_config.pointer("/text_config/max_position_embeddings"),
        raw_config.pointer("/max_position_embeddings"),
        raw_config.pointer("/text_config/sliding_window"),
        raw_config.pointer("/sliding_window"),
        raw_config.pointer("/text_config/model_max_length"),
        raw_config.pointer("/model_max_length"),
        raw_config.pointer("/seq_length"),
    ]
    .into_iter()
    .flatten()
    .find_map(|value| value.as_u64().map(|v| v as usize))
}

pub(super) fn preferred_dtype(device: &Device) -> DType {
    if device.is_cuda() || device.is_metal() {
        DType::BF16
    } else {
        DType::F32
    }
}

pub(super) fn device_label(device: &Device) -> &'static str {
    if device.is_cuda() {
        "cuda"
    } else if device.is_metal() {
        "metal"
    } else {
        "cpu"
    }
}

pub(super) fn dtype_label(dtype: DType) -> &'static str {
    match dtype {
        DType::BF16 => "bf16",
        DType::F16 => "f16",
        DType::F32 => "f32",
        DType::F64 => "f64",
        DType::U8 => "u8",
        DType::U32 => "u32",
        DType::I64 => "i64",
        _ => "unknown",
    }
}

pub(super) fn select_device() -> Result<Device> {
    #[cfg(feature = "cuda")]
    {
        let dev = Device::cuda_if_available(0)?;
        if !matches!(dev, Device::Cpu) {
            return Ok(dev);
        }
    }

    #[cfg(feature = "metal")]
    {
        let dev = Device::metal_if_available(0)?;
        if !matches!(dev, Device::Cpu) {
            return Ok(dev);
        }
    }

    Ok(Device::Cpu)
}
