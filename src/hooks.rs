// Hook declaration scaffolding for repo and protocol extensions.
//
// Discovers `.claude/hooks/` declarations, normalises them into
// RARA-owned HookDefinition objects, and surfaces them via
// /context and /status.
//
// Execution is explicitly disabled until permission and sandbox
// policy are defined.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

/// Hook phases aligned with Claude Code's lifecycle events.
/// RARA may normalise Claude hook files into these phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookPhase {
    /// When a new session begins.
    SessionStart,
    /// When the user sends a prompt.
    UserPromptSubmit,
    /// Before a tool is executed.
    PreToolUse,
    /// After a tool completes.
    PostToolUse,
    /// When the session stops.
    Stop,
}

impl HookPhase {
    /// Parse from Claude-style hook file names, e.g. "pre-tool-use" → PreToolUse.
    pub fn from_filename(name: &str) -> Option<Self> {
        match name {
            "session-start" | "session_start" => Some(Self::SessionStart),
            "user-prompt-submit" | "user_prompt_submit" => Some(Self::UserPromptSubmit),
            "pre-tool-use" | "pre_tool_use" => Some(Self::PreToolUse),
            "post-tool-use" | "post_tool_use" => Some(Self::PostToolUse),
            "stop" => Some(Self::Stop),
            _ => None,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::SessionStart => "session_start",
            Self::UserPromptSubmit => "user_prompt_submit",
            Self::PreToolUse => "pre_tool_use",
            Self::PostToolUse => "post_tool_use",
            Self::Stop => "stop",
        }
    }
}

/// A discovered and normalised hook declaration.
#[derive(Debug, Clone, Serialize)]
pub struct HookDefinition {
    /// Unique id derived from source path.
    pub id: String,
    /// Repository-relative source path.
    pub source_path: String,
    /// Declared hook phase.
    pub phase: HookPhase,
    /// Whether the hook could be fully parsed.
    pub parse_status: HookParseStatus,
    /// Hook body / handler content.
    pub body: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookParseStatus {
    Ok,
    /// File exists but could not be parsed or is empty.
    ParseError,
}

/// Discovers hook candidates and stores their normalised definitions.
/// Execution is currently disabled — this is discovery-only.
pub struct HookRegistry {
    pub hooks: BTreeMap<String, HookDefinition>,
    pub load_warnings: Vec<String>,
    /// All hook phases that have at least one registered hook.
    pub active_phases: Vec<HookPhase>,
}

impl HookRegistry {
    pub fn new() -> Self {
        Self {
            hooks: BTreeMap::new(),
            load_warnings: Vec::new(),
            active_phases: Vec::new(),
        }
    }

    /// Scan a directory for Claude-style hook files.
    /// Expected file names: `pre-tool-use.md`, `session-start.md`, etc.
    pub fn discover_from_dir(&mut self, dir: &Path) {
        if !dir.exists() {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.load_warnings
                    .push(format!("hook dir {}: {err}", dir.display()));
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(stem) = path.file_stem().and_then(|s| s.to_str()) else {
                continue;
            };
            let Some(phase) = HookPhase::from_filename(stem) else {
                continue;
            };

            let id = format!("hook-{}", path.display());
            let source_path = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();

            let body = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) => {
                    self.hooks.insert(
                        id.clone(),
                        HookDefinition {
                            id,
                            source_path,
                            phase,
                            parse_status: HookParseStatus::ParseError,
                            body: format!("read error: {err}"),
                        },
                    );
                    continue;
                }
            };

            let parse_status = if body.trim().is_empty() {
                HookParseStatus::ParseError
            } else {
                HookParseStatus::Ok
            };

            self.hooks.insert(
                id.clone(),
                HookDefinition {
                    id,
                    source_path,
                    phase,
                    parse_status,
                    body,
                },
            );
        }

        self.refresh_active_phases();
    }

    /// Discover hooks from a repository root directory.
    pub fn discover_repo_hooks(&mut self, repo_root: &Path) {
        let hooks_dir = repo_root.join(".claude").join("hooks");
        self.discover_from_dir(&hooks_dir);
    }

    fn refresh_active_phases(&mut self) {
        let mut phases: Vec<HookPhase> = self
            .hooks
            .values()
            .filter(|h| h.parse_status == HookParseStatus::Ok)
            .map(|h| h.phase)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        phases.sort_by_key(|p| phase_ordinal(*p));
        self.active_phases = phases;
    }

    /// For /context and /status: list each hook with phase, path, and parse status.
    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for hook in self.hooks.values() {
            let status = match hook.parse_status {
                HookParseStatus::Ok => "ok",
                HookParseStatus::ParseError => "parse_error",
            };
            lines.push(format!(
                "  {}  {}  {}  (disabled)",
                hook.phase.as_str(),
                hook.source_path,
                status
            ));
        }
        lines
    }
}

fn phase_ordinal(phase: HookPhase) -> u8 {
    match phase {
        HookPhase::SessionStart => 0,
        HookPhase::UserPromptSubmit => 1,
        HookPhase::PreToolUse => 2,
        HookPhase::PostToolUse => 3,
        HookPhase::Stop => 4,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_hook_phases_from_claude_filenames() {
        assert_eq!(
            HookPhase::from_filename("pre-tool-use"),
            Some(HookPhase::PreToolUse)
        );
        assert_eq!(
            HookPhase::from_filename("session-start"),
            Some(HookPhase::SessionStart)
        );
        assert_eq!(HookPhase::from_filename("stop"), Some(HookPhase::Stop));
        assert_eq!(HookPhase::from_filename("unknown"), None);
    }

    #[test]
    fn discovers_hooks_from_directory() {
        let dir = tempdir().expect("tempdir");
        let hooks_dir = dir.path().join(".claude").join("hooks");
        fs::create_dir_all(&hooks_dir).expect("mkdir");
        fs::write(
            hooks_dir.join("pre-tool-use.md"),
            "# Pre-tool use hook\nrun validation",
        )
        .expect("write");
        fs::write(hooks_dir.join("stop.md"), "# Stop hook\ncleanup").expect("write");
        fs::write(hooks_dir.join("unknown.md"), "ignored").expect("write");

        let mut registry = HookRegistry::new();
        registry.discover_from_dir(&hooks_dir);

        assert_eq!(registry.hooks.len(), 2);
        assert!(registry.active_phases.contains(&HookPhase::PreToolUse));
        assert!(registry.active_phases.contains(&HookPhase::Stop));
    }

    #[test]
    fn empty_hook_file_is_parse_error() {
        let dir = tempdir().expect("tempdir");
        let hooks_dir = dir.path().join(".claude").join("hooks");
        fs::create_dir_all(&hooks_dir).expect("mkdir");
        fs::write(hooks_dir.join("stop.md"), "").expect("write");

        let mut registry = HookRegistry::new();
        registry.discover_from_dir(&hooks_dir);

        let hook = registry.hooks.values().next().unwrap();
        assert_eq!(hook.parse_status, HookParseStatus::ParseError);
        assert!(registry.active_phases.is_empty());
    }
}
