use std::collections::HashSet;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use serde_json::Value;

use crate::agent::Message;
use crate::llm::{ContentBlock, LlmBackend, LlmTurnMetadata};
use crate::memory_store::{MemoryLabel, MemoryRecordSearchHit, NewMemoryRecord};

const MIN_DISTILLED_MEMORIES: usize = 2;
const MAX_DISTILLED_MEMORIES: usize = 8;
const DUPLICATE_SCORE_THRESHOLD: f32 = 0.92;

pub struct MemoryDistiller {
    backend: Arc<dyn LlmBackend>,
}

#[derive(Debug, Clone)]
pub struct DistilledMemoryDraft {
    pub title: String,
    pub content: String,
    pub labels: Vec<MemoryLabel>,
    pub importance: f32,
}

impl MemoryDistiller {
    pub fn new(backend: Arc<dyn LlmBackend>) -> Self {
        Self { backend }
    }

    pub async fn distill_thread_markdown(
        &self,
        thread_markdown: &str,
    ) -> Result<Vec<DistilledMemoryDraft>> {
        if thread_markdown.trim().is_empty() {
            return Ok(Vec::new());
        }

        let response = self
            .backend
            .ask_with_context(
                &[Message {
                    role: "user".to_string(),
                    content: serde_json::json!(distillation_prompt(thread_markdown)),
                }],
                &[],
                LlmTurnMetadata::execute(),
            )
            .await
            .context("distill thread memories")?;
        let text = response_text(&response.content);
        parse_distilled_memories(&text)
    }
}

pub fn dedupe_memory_drafts(
    drafts: Vec<DistilledMemoryDraft>,
    existing_hits: &[Vec<MemoryRecordSearchHit>],
) -> Vec<DistilledMemoryDraft> {
    let mut seen = HashSet::new();
    drafts
        .into_iter()
        .enumerate()
        .filter_map(|(index, draft)| {
            let hits = existing_hits
                .get(index)
                .map(Vec::as_slice)
                .unwrap_or_default();
            let content_key = normalize_dedupe_key(&draft.content);
            let title_key = normalize_dedupe_key(&draft.title);
            if content_key.is_empty() || !seen.insert(content_key.clone()) {
                return None;
            }
            let duplicate = hits.iter().any(|hit| {
                hit.score >= DUPLICATE_SCORE_THRESHOLD
                    || normalize_dedupe_key(&hit.record.content) == content_key
                    || normalize_dedupe_key(&hit.record.title) == title_key
            });
            (!duplicate).then_some(draft)
        })
        .collect()
}

pub fn new_memory_record_from_draft(
    draft: DistilledMemoryDraft,
    mut base: NewMemoryRecord,
) -> NewMemoryRecord {
    base.title = Some(draft.title);
    base.content = draft.content;
    base.labels = draft.labels;
    base.importance = draft.importance;
    base
}

fn distillation_prompt(thread_markdown: &str) -> String {
    format!(
        "{}{}",
        concat!(
            "Extract durable memories from this coding-agent thread.\n\n",
            "Rules:\n",
            "- Return only JSON with a top-level `memories` array.\n",
            "- Extract 2 to 8 memories when the thread contains enough durable material.\n",
            "- A memory must stand alone without the full conversation.\n",
            "- Prefer decisions, facts, procedures, insights, and reusable experiences.\n",
            "- Do not save transient status, command progress, or vague summaries.\n",
            "- Treat older memories or prior conclusions as historical context, not the current truth.\n",
            "- If the thread proves that a previous memory is stale or the current design is poor, extract the corrected durable fact or procedure instead of preserving the old state.\n",
            "- It is valid to record that the agent should create or improve tooling when the thread establishes that a missing tool blocks reliable future work.\n",
            "- Labels must use only: insight, decision, fact, procedure, experience.\n",
            "- Importance must be a number from 0.1 to 1.0.\n\n",
            "Schema:\n",
            "{{\"memories\":[{{\"title\":\"short title\",\"content\":\"markdown memory\",\"labels\":[\"decision\"],\"importance\":0.7}}]}}\n\n",
            "Thread:\n"
        ),
        thread_markdown
    )
}

fn response_text(content: &[ContentBlock]) -> String {
    content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            ContentBlock::ToolUse { .. } | ContentBlock::ProviderMetadata { .. } => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_distilled_memories(text: &str) -> Result<Vec<DistilledMemoryDraft>> {
    let value = parse_json_payload(text)?;
    let response: DistilledMemoryResponse =
        serde_json::from_value(value).context("parse distilled memory response")?;
    let mut drafts = response
        .memories
        .into_iter()
        .filter_map(ParsedDistilledMemory::into_draft)
        .take(MAX_DISTILLED_MEMORIES)
        .collect::<Vec<_>>();
    if !drafts.is_empty() && drafts.len() < MIN_DISTILLED_MEMORIES {
        bail!("thread distillation returned one memory; expected 2-8 or zero");
    }
    if drafts.len() > MAX_DISTILLED_MEMORIES {
        drafts.truncate(MAX_DISTILLED_MEMORIES);
    }
    Ok(drafts)
}

fn parse_json_payload(text: &str) -> Result<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        bail!("thread distillation returned empty response");
    }
    if let Ok(value) = serde_json::from_str(trimmed) {
        return Ok(value);
    }
    let start = trimmed
        .find('{')
        .context("distillation response did not contain a JSON object")?;
    let end = trimmed
        .rfind('}')
        .context("distillation response did not contain a complete JSON object")?;
    serde_json::from_str(&trimmed[start..=end])
        .context("parse JSON object from distillation response")
}

#[derive(Debug, Deserialize)]
struct DistilledMemoryResponse {
    memories: Vec<ParsedDistilledMemory>,
}

#[derive(Debug, Deserialize)]
struct ParsedDistilledMemory {
    title: Option<String>,
    content: Option<String>,
    #[serde(default)]
    labels: Vec<MemoryLabel>,
    importance: Option<f32>,
}

impl ParsedDistilledMemory {
    fn into_draft(self) -> Option<DistilledMemoryDraft> {
        let title = self.title?.trim().to_string();
        let content = self.content?.trim().to_string();
        if title.is_empty() || content.is_empty() {
            return None;
        }
        Some(DistilledMemoryDraft {
            title,
            content,
            labels: normalize_labels(self.labels),
            importance: self.importance.unwrap_or(0.5).clamp(0.1, 1.0),
        })
    }
}

fn normalize_labels(labels: Vec<MemoryLabel>) -> Vec<MemoryLabel> {
    let mut normalized = Vec::new();
    for label in labels {
        if !normalized.contains(&label) {
            normalized.push(label);
        }
    }
    if normalized.is_empty() {
        normalized.push(MemoryLabel::Experience);
    }
    normalized
}

fn normalize_dedupe_key(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::{DistilledMemoryDraft, dedupe_memory_drafts, parse_distilled_memories};
    use crate::memory_store::{
        MemoryLabel, MemoryRecord, MemoryRecordSearchHit, MemoryScope, MemorySource,
    };

    #[test]
    fn parses_json_distillation_response_with_label_defaults() {
        let drafts = parse_distilled_memories(
            r#"{
              "memories": [
                {
                  "title": "Path decision",
                  "content": "Use `/tmp/rara` for session shards.",
                  "labels": ["decision"],
                  "importance": 0.8
                },
                {
                  "title": "Review workflow",
                  "content": "Resolve review threads after applying comments.",
                  "labels": [],
                  "importance": 2.0
                }
              ]
            }"#,
        )
        .expect("parse drafts");

        assert_eq!(drafts.len(), 2);
        assert_eq!(drafts[0].labels, vec![MemoryLabel::Decision]);
        assert_eq!(drafts[1].labels, vec![MemoryLabel::Experience]);
        assert_eq!(drafts[1].importance, 1.0);
    }

    #[test]
    fn dedupes_drafts_against_existing_hits_and_same_batch() {
        let drafts = vec![
            DistilledMemoryDraft {
                title: "Path decision".to_string(),
                content: "Use /tmp/rara for session shards.".to_string(),
                labels: vec![MemoryLabel::Decision],
                importance: 0.8,
            },
            DistilledMemoryDraft {
                title: "Path decision".to_string(),
                content: "Use /tmp/rara for session shards.".to_string(),
                labels: vec![MemoryLabel::Decision],
                importance: 0.8,
            },
            DistilledMemoryDraft {
                title: "Review workflow".to_string(),
                content: "Resolve review threads after applying comments.".to_string(),
                labels: vec![MemoryLabel::Procedure],
                importance: 0.7,
            },
        ];
        let existing_hits = vec![
            vec![MemoryRecordSearchHit {
                record: memory_record(
                    "existing-1",
                    "Path decision",
                    "Use /tmp/rara for session shards.",
                ),
                score: 0.5,
                vector_distance: None,
                fts_score: None,
            }],
            Vec::new(),
            Vec::new(),
        ];

        let kept = dedupe_memory_drafts(drafts, &existing_hits);

        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].title, "Review workflow");
    }

    fn memory_record(id: &str, title: &str, content: &str) -> MemoryRecord {
        MemoryRecord {
            id: id.to_string(),
            title: title.to_string(),
            content: content.to_string(),
            labels: vec![MemoryLabel::Decision],
            importance: 0.8,
            pinned: false,
            source: MemorySource::ThreadDistill,
            scope: MemoryScope::Thread,
            session_id: None,
            thread_id: None,
            source_span: None,
            created_at_unix_seconds: 1,
            updated_at_unix_seconds: 1,
        }
    }
}
