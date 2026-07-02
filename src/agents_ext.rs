// Imported agent definitions from RARA repo extensions.
//
// Discovers `.rara/agents/*.md` plus legacy `.claude/agents/*.md` files that
// use the Claude-compatible frontmatter format, normalises them into RARA-owned
// ImportedAgentProfile objects, and surfaces them through /status.

use std::collections::HashMap;
use std::path::Path;

use serde::Serialize;

use crate::tools::agent::{AgentDefinitionLoadRecord, discover_workspace_agent_definition_records};

/// A Claude-compatible imported agent definition, normalised into
/// a RARA-owned profile object.
#[derive(Debug, Clone, Serialize)]
pub struct ImportedAgentProfile {
    /// Canonical agent id from parsed frontmatter; falls back to file stem on parse errors.
    pub id: String,
    /// Human-readable label for status display.
    pub label: String,
    /// Repository-relative source path.
    pub source_path: String,
    /// Where this agent originated.
    pub source_kind: String,
    /// Markdown body after the frontmatter block.
    pub prompt_body: String,
    /// Short description from parsed frontmatter, or parse error detail.
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

/// Discovers RARA-local Claude-compatible agent profiles and stores their
/// normalised definitions for status display.
pub struct AgentRegistry {
    pub agents: HashMap<String, ImportedAgentProfile>,
}

impl AgentRegistry {
    pub fn new() -> Self {
        Self {
            agents: HashMap::new(),
        }
    }

    /// Discover agent profiles from a repository root directory.
    pub fn discover_repo_agents(&mut self, repo_root: &Path) {
        for record in discover_workspace_agent_definition_records(repo_root) {
            self.insert_record(record, repo_root);
        }
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

    fn insert_record(&mut self, record: AgentDefinitionLoadRecord, base_dir: &Path) {
        let fallback_id = record.id.clone();
        let source_path = record
            .source_path
            .strip_prefix(base_dir)
            .unwrap_or(&record.source_path)
            .display()
            .to_string();
        let profile = match record.definition {
            Some(definition) => {
                let id = definition.name.clone();
                ImportedAgentProfile {
                    id: id.clone(),
                    label: id,
                    source_path,
                    source_kind: "rara_agent".to_string(),
                    prompt_body: definition.system_prompt,
                    description: definition.description,
                    parse_status: AgentParseStatus::Ok,
                }
            }
            None => ImportedAgentProfile {
                id: fallback_id.clone(),
                label: fallback_id,
                source_path,
                source_kind: "rara_agent".to_string(),
                prompt_body: String::new(),
                description: record.error.unwrap_or_else(|| "parse error".to_string()),
                parse_status: AgentParseStatus::ParseError,
            },
        };
        self.agents.insert(profile.id.clone(), profile);
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn discovers_agents_from_directory() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".rara").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(
            agents_dir.join("code-reviewer.md"),
            r#"---
name: code-reviewer
description: Reviews pull requests for correctness and style.
tools: [Read, Grep]
---

Review pull requests for correctness and style.
"#,
        )
        .expect("write");
        fs::write(
            agents_dir.join("test-writer.md"),
            r#"---
name: test-writer
description: Generates unit tests for new code paths.
tools: [Read, Write]
---

Generate unit tests for new code paths.
"#,
        )
        .expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_repo_agents(dir.path());

        assert_eq!(registry.agents.len(), 2);

        let reviewer = registry.agents.get("code-reviewer").expect("reviewer");
        assert_eq!(reviewer.label, "code-reviewer");
        assert_eq!(reviewer.source_kind, "rara_agent");
        assert_eq!(reviewer.parse_status, AgentParseStatus::Ok);
        assert!(reviewer.description.contains("Reviews pull requests"));
        assert_eq!(reviewer.source_path, ".rara/agents/code-reviewer.md");
    }

    #[test]
    fn empty_agent_file_is_parse_error() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".rara").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(agents_dir.join("empty.md"), "").expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_repo_agents(dir.path());

        let agent = registry.agents.get("empty").expect("empty agent");
        assert_eq!(agent.parse_status, AgentParseStatus::ParseError);
    }

    #[test]
    fn discover_repo_agents_prefers_rara_agents() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".claude").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(
            agents_dir.join("helper.md"),
            r#"---
name: helper
description: Legacy helper.
---

Legacy prompt.
"#,
        )
        .expect("write");

        let rara_agents_dir = dir.path().join(".rara").join("agents");
        fs::create_dir_all(&rara_agents_dir).expect("mkdir");
        fs::write(
            rara_agents_dir.join("helper.md"),
            r#"---
name: helper
description: RARA helper.
---

RARA prompt.
"#,
        )
        .expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_repo_agents(dir.path());

        let agent = registry.agents.get("helper").expect("helper");
        assert_eq!(agent.label, "helper");
        assert_eq!(agent.description, "RARA helper.");
        assert_eq!(agent.source_path, ".rara/agents/helper.md");
        assert_eq!(agent.parse_status, AgentParseStatus::Ok);
    }

    #[test]
    fn discover_repo_agents_uses_frontmatter_name_as_id() {
        let dir = tempdir().expect("tempdir");
        let agents_dir = dir.path().join(".rara").join("agents");
        fs::create_dir_all(&agents_dir).expect("mkdir");
        fs::write(
            agents_dir.join("reviewer-file.md"),
            r#"---
name: canonical-reviewer
description: Review code.
---

Review prompt.
"#,
        )
        .expect("write");

        let mut registry = AgentRegistry::new();
        registry.discover_repo_agents(dir.path());

        assert!(!registry.agents.contains_key("reviewer-file"));
        let agent = registry
            .agents
            .get("canonical-reviewer")
            .expect("canonical reviewer");
        assert_eq!(agent.id, "canonical-reviewer");
        assert_eq!(agent.label, "canonical-reviewer");
        assert_eq!(agent.source_path, ".rara/agents/reviewer-file.md");
        assert_eq!(agent.parse_status, AgentParseStatus::Ok);
    }
}
