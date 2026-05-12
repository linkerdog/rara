/// Background auto-memory extraction after each turn.
///
/// After every 5 turns, collects recent user/assistant messages
/// and uses the LLM backend to extract durable facts, then writes
/// them to the MemoryStore (JSON companion file + LanceDB index).
/// No embedding model is required — the JSON file stores full content
/// and LanceDB insertion is best-effort.
use std::sync::Arc;

use tokio::sync::Mutex;

use crate::agent::Message;
use crate::llm::LlmBackend;
use crate::memory_store::{MemoryLabel, MemoryScope, MemorySource, MemoryStore, NewMemoryRecord};

const EXTRACTION_INTERVAL: u64 = 5;

const EXTRACTION_INSTRUCTION: &str = r#"You are a memory-extraction routine. Read the conversation below and extract durable facts that will be useful for future turns. Output one fact per line, plain text, no markdown bullets. If nothing is worth remembering, output nothing. Focus on: decisions made, constraints discovered, preferences stated, and technical context that is likely to persist across sessions."#;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_chars);
        format!("{}…", &s[..end])
    }
}

/// Fact extraction state, stored once and reused across turns.
struct AutoMemoryExtractor {
    triggered_count: u64,
}

impl AutoMemoryExtractor {
    fn new() -> Self {
        Self { triggered_count: 0 }
    }

    fn maybe_trigger(
        &mut self,
        backend: Arc<dyn LlmBackend>,
        store: Arc<MemoryStore>,
        messages: Vec<Message>,
    ) {
        if messages.is_empty() {
            return;
        }
        self.triggered_count += 1;

        let backend = backend.clone();
        let store = store.clone();
        tokio::spawn(async move {
            let result = match backend.summarize(&messages, EXTRACTION_INSTRUCTION).await {
                Ok(r) => r,
                Err(_e) => {
                    return;
                }
            };

            for line in result.lines() {
                let content = line.trim();
                if content.is_empty() {
                    continue;
                }

                let record = NewMemoryRecord {
                    title: Some(truncate(content, 80)),
                    content: format!("- {content}"),
                    labels: vec![MemoryLabel::Fact],
                    importance: 0.5,
                    pinned: false,
                    scope: MemoryScope::User,
                    source: MemorySource::AutoMemory,
                    session_id: None,
                    thread_id: None,
                    source_span: None,
                };
                if let Err(_e) = store.insert_text_only(record).await {}
            }
        });
    }
}

/// Hook called from tasks.rs after every completed turn.
/// Checks if enough turns have passed and spawns background extraction.
pub fn maybe_auto_memory(app: &crate::tui::state::TuiApp, agent: &crate::agent::Agent) {
    let completed = app.committed_turns.len() as u64;
    if completed == 0 || completed % EXTRACTION_INTERVAL != 0 {
        return;
    }

    // Collect recent user/assistant messages in chronological order.
    let messages: Vec<Message> = app
        .committed_turns
        .iter()
        .rev()
        .take(EXTRACTION_INTERVAL as usize)
        .flat_map(|t| &t.entries)
        .filter(|e| e.role == "user" || e.role == "assistant")
        .map(|e| Message {
            role: e.role.clone(),
            content: serde_json::Value::String(e.message.clone()),
        })
        .rev()
        .collect();

    use std::sync::OnceLock;
    static EXTRACTOR: OnceLock<Mutex<AutoMemoryExtractor>> = OnceLock::new();
    let extractor = EXTRACTOR.get_or_init(|| Mutex::new(AutoMemoryExtractor::new()));

    // Spawn extraction without holding the lock across the LLM call.
    // Grab a clone of the messages and trigger from inside the lock.
    let mut guard = extractor.blocking_lock();
    guard.maybe_trigger(
        agent.llm_backend.clone(),
        agent.memory_store.clone(),
        messages,
    );
}
