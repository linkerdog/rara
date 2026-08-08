use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

/// Origin of a discovered Claude Code plugin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PluginSource {
    Builtin(PathBuf),
    User(PathBuf),
    Project(PathBuf),
    Cli(PathBuf),
    Directory(PathBuf),
}

impl PluginSource {
    pub fn path(&self) -> &Path {
        match self {
            Self::Builtin(path)
            | Self::User(path)
            | Self::Project(path)
            | Self::Cli(path)
            | Self::Directory(path) => path,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Builtin(_) => "builtin",
            Self::User(_) => "user",
            Self::Project(_) => "project",
            Self::Cli(_) => "cli",
            Self::Directory(_) => "directory",
        }
    }
}

/// A loaded Claude Code plugin.
#[derive(Debug, Clone)]
pub struct Plugin {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub root: PathBuf,
    pub source: PluginSource,
    pub hooks: Vec<HookHandler>,
    pub mcp_config: Option<McpConfig>,
    pub load_warnings: Vec<String>,
}

/// A single hook handler from hooks.json.
#[derive(Debug, Clone, Deserialize)]
pub struct HookHandler {
    #[serde(default)]
    pub r#type: String,
    #[serde(default)]
    pub command: String,
    #[serde(default)]
    pub timeout: u64,
    #[serde(default)]
    pub matcher: Option<String>,
    #[serde(default)]
    pub once: bool,
}

/// Registered hook handler with its event binding.
#[derive(Debug, Clone)]
pub struct RegisteredHook {
    pub event: HookEvent,
    pub handler: HookHandler,
    pub plugin_name: String,
    pub plugin_root: PathBuf,
}

/// Hook lifecycle events matching hooks.json keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    Stop,
    PostToolUse,
    PreToolUse,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
    GoalCreated,
    GoalCompleted,
}

impl HookEvent {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Stop => "Stop",
            Self::PostToolUse => "PostToolUse",
            Self::PreToolUse => "PreToolUse",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::GoalCreated => "GoalCreated",
            Self::GoalCompleted => "GoalCompleted",
        }
    }
}

/// Parsed .mcp.json content.
#[derive(Debug, Clone, Deserialize)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub mcp_servers: BTreeMap<String, McpServer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct McpServer {
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
}
