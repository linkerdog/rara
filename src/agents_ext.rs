// Imported agent definitions from Claude-style repo extensions.
//
// Discovers `.claude/agents/*.md` files, normalises them into
// RARA-owned ImportedAgentProfile objects, and surfaces them.
//
// Execution is explicitly deferred — these are discovery and
// normalisation only. Execution will go through RARA's
// thread/sub-agent model when implemented.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use serde::Serialize;

/// A Claude-style imported agent definition, normalised into
/// a RARA-owned profile object.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedAgentProfile {
    /// Unique id derived from source file stem.
    pub id: String,
    /// Human-readable label (from first heading or filename).
    pub label: String,
    /// Repository-relative source path.
    pub source_path: String,
    /// Where this agent originated (currently always "claude_agent").
    pub source_kind: String,
    /// The raw markdown body.
    pub prompt_body: String,
    /// Short description extracted from the first non-heading line.
    pub description: String,
    /// Whether the file could be read successfully.
    pub parse_status: AgentParseStatus,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentParseStatus {
    Ok,
    /// File exists but could not be read or is empty.
    ParseError,
}

/// Discovers Claude-style agent profiles and stores their normalised
/// definitions. Execution is currently not wired — this is discovery-only.
pub struct AgentRegistry {
    pub agents: HashMap<String, ImportedAgentProfile>,
    pub load_warnings: Vec<String>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
            load_warnings: Vec::new(),
        }
    }

    /// Scan a directory for `.md` agent definition files.
    pub fn discover_from_dir(&mut self, dir: &Path) {
        if !dir.exists() {
            return;
        }
        let entries = match fs::read_dir(dir) {
            Ok(entries) => entries,
            Err(err) => {
                self.load_warnings
                    .push(format!("agent dir {}: {err}", dir.display()));
                return;
            }
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let Some(ext) = path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if ext != "md" {
                continue;
            }

            let id = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string();
            let source_path = path
                .strip_prefix(dir)
                .unwrap_or(&path)
                .display()
                .to_string();

            let body = match fs::read_to_string(&path) {
                Ok(content) => content,
                Err(err) => {
                    self.agents.insert(
                        id.clone(),
                        ImportedAgentProfile {
                            id,
                            label: source_path.clone(),
                            source_path,
                            source_kind: "claude_agent".to_string(),
                            prompt_body: String::new(),
                            description: format!("read error: {err}"),
                            parse_status: AgentParseStatus::ParseError,
                        },
                    );
                    continue;
                }
            };

            if body.trim().is_empty() {
                self.agents.insert(
                    id.clone(),
                    ImportedAgentProfile {
                        id,
                        label: source_path.clone(),
                        source_path,
                        source_kind: "claude_agent".to_string(),
                        prompt_body: String::new(),
                        description: "empty file".to_string(),
                        parse_status: AgentParseStatus::ParseError,
                    },
                );
                continue;
            }

            let label = extract_agent_label(&body, &id);
            let description = extract_agent_description(&body);

            self.agents.insert(
                id.clone(),
                ImportedAgentProfile {
                    id,
                    label,
                    source_path,
                    source_kind: "claude_agent".to_string(),
                    prompt_body: body,
                    description,
                    parse_status: AgentParseStatus::Ok,
                },
            );
        }
    }

    /// Discover agent profiles from a repository root directory.
    pub fn discover_repo_agents(&mut self, repo_root: &Path) {
        let agents_dir = repo_root.join(".claude").join("agents");
        self.discover_from_dir(&agents_dir);
    }

    /// For /context and /status: list each discovered agent with path and status.
    pub fn status_lines(&self) -> Vec<String> {
        let mut lines = Vec::new();
        for agent in self.agents.values() {
            let status = match agent.parse_status {
                AgentParseStatus::Ok => "ok",
                AgentParseStatus::ParseError => "parse_error",
            };
            lines.push(format!(
                "  {}  {}  {}  (disabled)",
                agent.id, agent.source_path, status
            ));
        }
        if lines.is_empty() {
            lines.push("  (none)".to_string());
        }
        lines.sort();
        lines
    }
}

/// Extract a human-readable label from the first Markdown heading,
/// falling back to the file stem.
fn extract_agent_label(body: &str, fallback_id: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let rest = rest.trim_start_matches('#').trim();
            if !rest.is_empty() {
                return rest.to_string();
            }
        }
    }
    fallback_id.to_string()
}

/// Extract a short description from the first non-empty, non-heading line.
fn extract_agent_description(body: &str) -> String {
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let desc: String = trimmed.chars().take(120).collect();
        if desc.len() < trimmed.len() {
            return format!("{desc}...");
        }
        return desc;
    }
    "no description".to_string()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn discovers_agents_from_directory() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(
            agents_dir.join("code-reviewer.md"),
            "# Code Reviewer\n\nReviews pull requests for correctness and style.\n",
        )
        .expect("write");
        fs::write(
            agents_dir.join("test-writer.md"),
            "# Test Writer\n\nGenerates unit tests for new code paths.\n",
        )
        .expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_from_dir(&agents_dir);

        assert_eq!(registry.agents.len(), 2);

        let reviewer = registry.agents.get("code-reviewer").expect("reviewer");
        assert_eq!(reviewer.label, "Code Reviewer");
        assert_eq!(reviewer.source_kind, "claude_agent");
        assert_eq!(reviewer.parse_status, AgentParseStatus::Ok);
        assert!(reviewer.description.contains("Reviews pull requests"));
    }

    #[test]
    fn empty_agent_file_is_parse_error() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(agents_dir.join("empty.md"), "").expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_from_dir(&agents_dir);

        let agent = registry.agents.get("empty").expect("empty agent");
        assert_eq!(agent.parse_status, AgentParseStatus::ParseError);
    }

    #[test]
    fn agent_label_falls_back_to_filename() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(agents_dir.join("helper.md"), "Just a helpful assistant.\n").expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_from_dir(&agents_dir);

        let agent = registry.agents.get("helper").expect("helper");
        // First non-heading line is the description; no heading → fallback to filename
        assert_eq!(agent.label, "helper");
        assert_eq!(agent.description, "Just a helpful assistant.");
        assert_eq!(agent.parse_status, AgentParseStatus::Ok);
    }
}
