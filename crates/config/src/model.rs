use std::collections::{BTreeMap, hash_map::DefaultHasher};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

use anyhow::Result;
use codex_execpolicy::{PolicyParser, blocking_append_allow_prefix_rule};
use secrecy::{ExposeSecret, SecretString};
use serde::{Deserialize, Serialize};

use crate::defaults::{
    DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_MODEL, DEFAULT_CONSOLIDATION_MIN_HOURS,
    DEFAULT_CONSOLIDATION_MIN_SESSIONS, DEFAULT_CONSOLIDATION_SCAN_INTERVAL_MINUTES,
    DEFAULT_DEEPSEEK_BASE_URL, DEFAULT_DEEPSEEK_MODEL, DEFAULT_GEMINI_BASE_URL,
    DEFAULT_KIMI_BASE_URL, DEFAULT_KIMI_CODING_BASE_URL, DEFAULT_KIMI_CODING_MODEL,
    DEFAULT_KIMI_MODEL, DEFAULT_OPENAI_COMPATIBLE_BASE_URL, DEFAULT_OPENAI_COMPATIBLE_MODEL,
    DEFAULT_OPENROUTER_BASE_URL, DEFAULT_OPENROUTER_MODEL, DEFAULT_REASONING_SUMMARY,
    should_apply_codex_base_url, should_reset_codex_model,
};
use crate::mcp::{McpRegistry, load_mcp_registry};
use crate::migration::migrate_reasoning_summary;
use crate::multi_agent::MultiAgentPolicy;
use crate::provider_surface::{ConfigValueSource, EffectiveProviderSurface, ResolvedProviderValue};
use crate::secrets::{deserialize_secret_option, serialize_secret_option};
use crate::serde_helpers::{normalize_optional_string, normalize_reasoning_summary};

/// Background memory consolidation (dream) settings.
///
/// Consolidation runs as a background sub-agent that reads session logs,
/// extracts durable facts, and merges them into the project memory index.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MemoryConsolidationConfig {
    /// Model name.  `"inherit"` (default) uses the main model.
    #[serde(default = "default_consolidation_model")]
    pub model: String,
    /// Reasoning effort (low / medium / high).
    #[serde(default = "default_consolidation_reasoning_effort")]
    pub reasoning_effort: String,
    /// Minimum hours since last consolidation before next is eligible.
    pub min_hours_since_last: u64,
    /// Minimum new touching sessions before triggering.
    pub min_new_sessions: u64,
    /// Minimum scan interval in minutes.
    pub scan_interval_minutes: u64,
}

impl Default for MemoryConsolidationConfig {
    fn default() -> Self {
        Self {
            model: default_consolidation_model(),
            reasoning_effort: default_consolidation_reasoning_effort(),
            min_hours_since_last: DEFAULT_CONSOLIDATION_MIN_HOURS,
            min_new_sessions: DEFAULT_CONSOLIDATION_MIN_SESSIONS,
            scan_interval_minutes: DEFAULT_CONSOLIDATION_SCAN_INTERVAL_MINUTES,
        }
    }
}

impl MemoryConsolidationConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_consolidation_model() -> String {
    super::defaults::DEFAULT_CONSOLIDATION_MODEL.into()
}

fn default_consolidation_reasoning_effort() -> String {
    super::defaults::DEFAULT_CONSOLIDATION_REASONING_EFFORT.into()
}

/// Controls whether fuzzy path matches enter automatic retrieval context.
///
/// `paths_only` keeps file search as a low-priority candidate source that only
/// exposes path/provenance metadata. It does not read or inject file contents.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ContextFileSearchPolicy {
    Off,
    #[default]
    PathsOnly,
}

impl ContextFileSearchPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

/// Deprecated compatibility setting. Local semantic memory is delegated to
/// official Mem, so this value is parsed but no longer starts a bundled
/// embedding runtime.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalEmbeddingPolicy {
    #[default]
    Off,
    Auto,
    Provider,
    Local,
}

impl LocalEmbeddingPolicy {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfigState {
    #[serde(
        default,
        serialize_with = "serialize_secret_option",
        deserialize_with = "deserialize_secret_option"
    )]
    pub api_key: Option<SecretString>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(default, alias = "utility_model")]
    pub auxiliary_model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_summary: Option<String>,
    pub revision: Option<String>,
    pub thinking: Option<bool>,
    pub num_ctx: Option<u32>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum OpenAiEndpointKind {
    #[default]
    Custom,
    Deepseek,
    Kimi,
    KimiCoding,
    Openrouter,
}

impl OpenAiEndpointKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Custom => "Custom endpoint",
            Self::Deepseek => "DeepSeek",
            Self::Kimi => "Moonshot AI",
            Self::KimiCoding => "Kimi For Coding",
            Self::Openrouter => "OpenRouter",
        }
    }

    pub fn default_profile_id(self) -> &'static str {
        match self {
            Self::Custom => "custom-default",
            Self::Deepseek => "deepseek-default",
            Self::Kimi => "kimi-default",
            Self::KimiCoding => "kimi-coding-default",
            Self::Openrouter => "openrouter-default",
        }
    }

    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Custom => DEFAULT_OPENAI_COMPATIBLE_BASE_URL,
            Self::Deepseek => DEFAULT_DEEPSEEK_BASE_URL,
            Self::Kimi => DEFAULT_KIMI_BASE_URL,
            Self::KimiCoding => DEFAULT_KIMI_CODING_BASE_URL,
            Self::Openrouter => DEFAULT_OPENROUTER_BASE_URL,
        }
    }

    pub fn default_model(self) -> &'static str {
        match self {
            Self::Custom => DEFAULT_OPENAI_COMPATIBLE_MODEL,
            Self::Deepseek => DEFAULT_DEEPSEEK_MODEL,
            Self::Kimi => DEFAULT_KIMI_MODEL,
            Self::KimiCoding => DEFAULT_KIMI_CODING_MODEL,
            Self::Openrouter => DEFAULT_OPENROUTER_MODEL,
        }
    }

    fn from_legacy_provider(provider: &str) -> Option<Self> {
        match provider {
            "openai-compatible" => Some(Self::Custom),
            "deepseek" => Some(Self::Deepseek),
            "kimi" => Some(Self::Kimi),
            "kimi-coding" => Some(Self::KimiCoding),
            "openrouter" => Some(Self::Openrouter),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct OpenAiEndpointProfile {
    pub id: String,
    pub label: String,
    pub kind: OpenAiEndpointKind,
    #[serde(
        default,
        serialize_with = "serialize_secret_option",
        deserialize_with = "deserialize_secret_option"
    )]
    pub api_key: Option<SecretString>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    #[serde(default, alias = "utility_model")]
    pub auxiliary_model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub reasoning_summary: Option<String>,
    pub revision: Option<String>,
}

#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
pub struct SandboxWorkspaceWriteConfig {
    #[serde(default = "default_true")]
    pub network_access: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SandboxWorkspaceWriteConfig {
    fn default() -> Self {
        Self {
            network_access: true,
        }
    }
}

impl SandboxWorkspaceWriteConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TuiConfig {
    #[serde(default, skip_serializing_if = "TuiThemeConfig::is_default")]
    pub theme: TuiThemeConfig,
}

impl TuiConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(default)]
pub struct BuiltinPluginConfig {
    #[serde(default, skip_serializing_if = "NowledgeMemPluginConfig::is_default")]
    pub nowledge_mem: NowledgeMemPluginConfig,
}

impl BuiltinPluginConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum NowledgeMemMode {
    #[default]
    Local,
    Cloud,
}

pub const DEFAULT_NOWLEDGE_MEM_CLOUD_URL: &str = "https://cloud.nowledge.co";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct NowledgeMemPluginConfig {
    #[serde(default, skip_serializing_if = "NowledgeMemMode::is_default")]
    pub mode: NowledgeMemMode,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub url: String,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        serialize_with = "serialize_secret_option",
        deserialize_with = "deserialize_secret_option"
    )]
    pub api_key: Option<SecretString>,
    #[serde(
        default = "default_nowledge_mem_api_key_env_var",
        skip_serializing_if = "is_default_nowledge_mem_api_key_env_var"
    )]
    pub api_key_env_var: String,
    #[serde(
        default = "default_nowledge_mem_space_id_env_var",
        skip_serializing_if = "is_default_nowledge_mem_space_id_env_var"
    )]
    pub space_id_env_var: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub http_headers: BTreeMap<String, String>,
}

impl PartialEq for NowledgeMemPluginConfig {
    fn eq(&self, other: &Self) -> bool {
        self.mode == other.mode
            && self.enabled == other.enabled
            && self.url == other.url
            && self.api_key() == other.api_key()
            && self.api_key_env_var == other.api_key_env_var
            && self.space_id_env_var == other.space_id_env_var
            && self.http_headers == other.http_headers
    }
}

impl Eq for NowledgeMemPluginConfig {}

impl Default for NowledgeMemPluginConfig {
    fn default() -> Self {
        Self {
            mode: NowledgeMemMode::Local,
            enabled: true,
            url: default_nowledge_mem_mcp_url(),
            api_key: None,
            api_key_env_var: default_nowledge_mem_api_key_env_var(),
            space_id_env_var: default_nowledge_mem_space_id_env_var(),
            http_headers: BTreeMap::new(),
        }
    }
}

impl NowledgeMemPluginConfig {
    pub fn mcp_url(&self) -> String {
        match self.mode {
            NowledgeMemMode::Local => self.url.clone(),
            NowledgeMemMode::Cloud => {
                let configured_url = self.url.trim_end_matches('/');
                let base = if configured_url.is_empty()
                    || configured_url == default_nowledge_mem_mcp_url().trim_end_matches('/')
                {
                    DEFAULT_NOWLEDGE_MEM_CLOUD_URL
                } else {
                    configured_url
                };
                if base.ends_with("/mcp") {
                    format!("{base}/")
                } else if base.ends_with("/remote-api") {
                    format!("{base}/mcp/")
                } else {
                    format!("{base}/remote-api/mcp/")
                }
            }
        }
    }

    pub fn api_url(&self) -> String {
        let mcp_url = self.mcp_url();
        mcp_url
            .trim_end_matches('/')
            .strip_suffix("/mcp")
            .unwrap_or_else(|| mcp_url.trim_end_matches('/'))
            .to_string()
    }

    pub fn env_http_headers(&self) -> Option<BTreeMap<String, String>> {
        if self.mode != NowledgeMemMode::Cloud {
            return None;
        }
        let mut headers = BTreeMap::from([
            ("Authorization".to_string(), self.api_key_env_var.clone()),
            ("X-NMEM-API-Key".to_string(), self.api_key_env_var.clone()),
        ]);
        if let Some(env_var) = &self.space_id_env_var {
            headers.insert("X-Nmem-Space-Id".to_string(), env_var.clone());
        }
        Some(headers)
    }

    pub fn api_key(&self) -> Option<&str> {
        self.api_key.as_ref().map(SecretString::expose_secret)
    }

    pub fn configured_space_id(&self) -> Option<String> {
        self.space_id_env_var
            .as_deref()
            .and_then(|name| std::env::var(name).ok())
    }

    pub fn set_api_key(&mut self, value: impl Into<String>) {
        self.api_key = Some(SecretString::from(value.into()));
    }

    pub fn clear_api_key(&mut self) {
        self.api_key = None;
    }

    pub fn mode_label(&self) -> &'static str {
        match self.mode {
            NowledgeMemMode::Local => "local",
            NowledgeMemMode::Cloud => "cloud",
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl NowledgeMemMode {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_nowledge_mem_mcp_url() -> String {
    "http://127.0.0.1:14242/mcp/".to_string()
}

fn default_nowledge_mem_api_key_env_var() -> String {
    "NMEM_API_KEY".to_string()
}

fn is_default_nowledge_mem_api_key_env_var(value: &str) -> bool {
    value == "NMEM_API_KEY"
}

fn default_nowledge_mem_space_id_env_var() -> Option<String> {
    Some("NMEM_SPACE".to_string())
}

fn is_default_nowledge_mem_space_id_env_var(value: &Option<String>) -> bool {
    value.as_deref() == Some("NMEM_SPACE")
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TuiThemeConfig {
    pub name: String,
    pub syntax_theme: Option<String>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tokens: BTreeMap<String, String>,
}

impl Default for TuiThemeConfig {
    fn default() -> Self {
        Self {
            name: default_tui_theme_name(),
            syntax_theme: None,
            tokens: BTreeMap::new(),
        }
    }
}

impl TuiThemeConfig {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

fn default_tui_theme_name() -> String {
    "nord".to_string()
}

mod rara_config;

pub use self::rara_config::RaraConfig;

pub struct ConfigManager {
    pub path: PathBuf,
}

impl ConfigManager {
    pub fn new() -> Result<Self> {
        Self::new_for_rara_home(ensure_rara_home_dir()?)
    }

    pub fn new_for_rara_home(rara_home: PathBuf) -> Result<Self> {
        fs::create_dir_all(&rara_home)?;
        Ok(Self {
            path: rara_home.join("config.json"),
        })
    }

    pub fn load(&self) -> Result<RaraConfig> {
        match fs::read_to_string(&self.path) {
            Ok(content) => {
                let mut config: RaraConfig = serde_json::from_str(&content).map_err(|err| {
                    anyhow::anyhow!("failed to parse {}: {err}", self.path.display())
                })?;
                config.migrate_legacy_provider_state();
                Ok(config)
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default_config()),
            Err(err) => Err(err.into()),
        }
    }

    pub fn save(&self, config: &RaraConfig) -> Result<()> {
        let content = serde_json::to_string_pretty(config)?;
        fs::write(&self.path, content)?;
        Ok(())
    }

    pub fn load_mcp_registry_for_project(&self, project_root: &Path) -> Result<McpRegistry> {
        load_mcp_registry(&self.config_toml_path(), project_root)
    }

    pub fn config_toml_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("config.toml")
    }

    pub fn rules_path(&self) -> PathBuf {
        self.path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("rules")
            .join("default.rules")
    }

    pub fn load_allowed_command_prefixes(&self) -> Result<Vec<String>> {
        let path = self.rules_path();
        let content = match fs::read_to_string(&path) {
            Ok(content) => content,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => return Err(err.into()),
        };
        let mut parser = PolicyParser::new();
        parser.parse(&path.display().to_string(), &content)?;
        let policy = parser.build();
        Ok(policy
            .get_allowed_prefixes()
            .into_iter()
            .map(|prefix| prefix.join(" "))
            .collect())
    }

    pub fn save_allowed_command_prefixes(&self, prefixes: &[String]) -> Result<()> {
        let path = self.rules_path();
        let mut requested = Vec::new();
        for prefix in prefixes {
            if requested.contains(prefix) {
                continue;
            }
            let tokens: Vec<String> = prefix.split_whitespace().map(str::to_string).collect();
            if tokens.is_empty() {
                continue;
            }
            blocking_append_allow_prefix_rule(&path, &tokens)?;
            requested.push(prefix.clone());
        }
        Ok(())
    }

    fn default_config() -> RaraConfig {
        RaraConfig {
            provider: "mock".to_string(),
            reasoning_summary: Some(DEFAULT_REASONING_SUMMARY.to_string()),
            thinking: Some(true),
            ..Default::default()
        }
    }
}

pub fn rara_home_dir() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .ok_or_else(|| anyhow::anyhow!("HOME or USERPROFILE environment variable not set"))?;
    Ok(home.join(".rara"))
}

pub fn ensure_rara_home_dir() -> Result<PathBuf> {
    let rara_home = rara_home_dir()?;
    fs::create_dir_all(&rara_home)?;
    Ok(rara_home)
}

pub fn workspace_data_dir_for(root: &Path) -> Result<PathBuf> {
    let rara_home = ensure_rara_home_dir()?;
    workspace_data_dir_for_home(root, &rara_home)
}

pub fn workspace_data_dir_for_home(root: &Path, rara_home: &Path) -> Result<PathBuf> {
    let canonical_root = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let slug = workspace_slug(&canonical_root);
    let hash = stable_path_hash(&canonical_root);
    let dir = rara_home
        .join("workspaces")
        .join(format!("{slug}-{hash:016x}"));
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

fn resolve_provider_value<'a>(
    provider_value: Option<&'a str>,
    legacy_value: Option<&'a str>,
    environment_value: Option<&'a str>,
    default_value: Option<&'a str>,
) -> ResolvedProviderValue<'a> {
    if let Some(value) = provider_value {
        return ResolvedProviderValue {
            value: Some(value),
            source: ConfigValueSource::ProviderState,
        };
    }
    if let Some(value) = legacy_value {
        return ResolvedProviderValue {
            value: Some(value),
            source: ConfigValueSource::LegacyGlobal,
        };
    }
    if let Some(value) = environment_value {
        return ResolvedProviderValue {
            value: Some(value),
            source: ConfigValueSource::Environment,
        };
    }
    if let Some(value) = default_value {
        return ResolvedProviderValue {
            value: Some(value),
            source: ConfigValueSource::BuiltInDefault,
        };
    }
    ResolvedProviderValue {
        value: None,
        source: ConfigValueSource::Unset,
    }
}

fn workspace_slug(root: &Path) -> String {
    let raw = root
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.trim().is_empty())
        .unwrap_or("workspace");
    let mut slug = String::new();
    let mut last_dash = false;
    for ch in raw.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch);
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    let slug = slug.trim_matches('-');
    if slug.is_empty() {
        "workspace".to_string()
    } else {
        slug.chars().take(40).collect()
    }
}

fn stable_path_hash(root: &Path) -> u64 {
    let mut hasher = DefaultHasher::new();
    root.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
#[path = "model_test.rs"]
mod tests;
