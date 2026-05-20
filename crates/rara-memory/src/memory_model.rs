//! Durable memory entry model with importance scoring and labels.
//!
//! Follows the Memory spec from Nowledge Mem:
//!   Importance: 0.1–1.0 (default 0.5)
//!   Labels: insight, decision, fact, procedure, experience
//!
//! Each `MemoryEntry` is the unit emitted by dream Phase 1 subagents
//! and consumed by the Phase 2 merge agent.

use serde::{Deserialize, Serialize};

/// Importance scale for a durable memory.
///
/// | Range     | Meaning    | Examples
/// |-----------|------------|----------
/// | 0.8 – 1.0 | Critical   | Architectural decisions, breakthrough discoveries, production incidents
/// | 0.5 – 0.7 | Useful     | Standard decisions, good insights, project learnings
/// | 0.1 – 0.4 | Background | Reference info, minor details, casual notes
pub type Importance = f32;

/// Standard memory labels (lowercase, hyphenated).
pub mod label {
    pub const INSIGHT: &str = "insight";
    pub const DECISION: &str = "decision";
    pub const FACT: &str = "fact";
    pub const PROCEDURE: &str = "procedure";
    pub const EXPERIENCE: &str = "experience";
}

/// A single durable memory extracted from sessions or contributed sources.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryEntry {
    /// Short summary (auto-generated or set manually).
    pub title: String,
    /// The knowledge itself.  Markdown.
    pub content: String,
    /// Categories for filtering / organisation.
    #[serde(default)]
    pub labels: Vec<String>,
    /// 0.1 – 1.0.  Affects search ranking and briefing priority.
    #[serde(default = "default_importance")]
    pub importance: Importance,
    /// Where this memory was extracted from (session id, team file, etc.).
    #[serde(default)]
    pub source: Option<String>,
    /// When the memory was created / extracted.
    #[serde(default)]
    pub created: Option<String>,
    /// Optional tags for LanceDB keyword search (space-separated).
    #[serde(default)]
    pub tags: Option<String>,
}

fn default_importance() -> Importance {
    0.5
}

impl MemoryEntry {
    /// A one-line pointer suitable for the `MEMORY.md` index.
    pub fn index_line(&self, topic_file: &str) -> String {
        let importance = match self.importance {
            i if i >= 0.8 => "★",
            i if i >= 0.5 => "·",
            _ => " ",
        };
        let tags = self
            .tags
            .as_deref()
            .map(|t| format!(" tags:{}", t))
            .unwrap_or_default();
        format!(
            "{importance} [{title}]({file}) — {summary}{tags}",
            importance = importance,
            title = self.title,
            file = topic_file,
            summary = content_summary(&self.content, 120),
            tags = tags,
        )
    }
}

/// Truncate content to at most `max_chars` characters, breaking at a word
/// boundary.
fn content_summary(content: &str, max_chars: usize) -> String {
    if content.len() <= max_chars {
        return content.to_string();
    }
    let end = content[..max_chars]
        .rfind(|c: char| c.is_whitespace())
        .unwrap_or(max_chars);
    format!("{}…", &content[..end])
}

/// Collection of memory entries produced by one subagent batch.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryBatch {
    /// Which subagent (or source) produced this batch.
    pub producer: String,
    /// The extracted entries.
    pub entries: Vec<MemoryEntry>,
    /// Whether the subagent found no new durable information.
    pub nothing_new: bool,
}
