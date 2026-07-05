use serde_json::{Value, json};

use super::{Agent, Message};
use crate::context::{
    MemoryRetrievalOrchestrator, MemorySelectionItemContextEntry, RetrievedMemoryRenderItem,
    SharedRuntimeContext, is_retrieved_memory_kind, render_retrieved_memory_context,
};

impl Agent {
    pub(super) async fn refresh_memory_retrieval_candidates(&mut self) {
        self.retrieved_memory_candidates = MemoryRetrievalOrchestrator::new_with_embedding_backend(
            self.embedding_backend.clone(),
            self.session_manager.clone(),
            self.memory_store.clone(),
        )
        .retrieve_for_history(&self.history)
        .await;
    }

    pub(super) fn refresh_file_search_candidates(&mut self) {
        if self.prompt_config.context_file_search == crate::config::ContextFileSearchPolicy::Off {
            self.file_search_candidates = Vec::new();
            return;
        }
        let query = latest_user_text(&self.history);
        if query.is_empty() {
            self.file_search_candidates = Vec::new();
            return;
        }
        self.file_search_candidates = self.file_search_provider.retrieval_candidates(&query, 64);
    }

    pub(super) fn selected_memory_context_text(
        runtime_context: &SharedRuntimeContext,
    ) -> Option<String> {
        let selected = runtime_context
            .retrieval
            .memory_selection
            .selected_items
            .iter()
            .filter(|item| is_retrieved_memory_kind(item.kind.as_str()))
            .collect::<Vec<_>>();

        render_selected_memory_context(selected.as_slice())
    }

    pub(super) fn prepend_memory_context_to_latest_user_message(
        messages: &mut [Message],
        memory_context: String,
    ) {
        let Some(message) = messages
            .iter_mut()
            .rfind(|message| message.role == "user" && is_user_text_request(message))
        else {
            return;
        };

        prepend_text_to_message(message, memory_context);
    }
}

fn prepend_text_to_message(message: &mut Message, text: String) {
    match &mut message.content {
        Value::Array(items) => items.insert(0, memory_text_block(text)),
        Value::String(existing) => {
            let original = std::mem::take(existing);
            *existing = format!("{text}\n\n{original}");
        }
        other => {
            let original = other.take();
            *other = json!([
                memory_text_block(text),
                {"type": "text", "text": original.to_string()}
            ]);
        }
    }
}

fn is_user_text_request(message: &Message) -> bool {
    if message
        .content
        .as_str()
        .is_some_and(|text| !text.trim().is_empty())
    {
        return true;
    }
    message.content.as_array().is_some_and(|items| {
        items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("text")
                && item
                    .get("text")
                    .and_then(Value::as_str)
                    .is_some_and(|text| !text.trim().is_empty())
        })
    })
}

fn memory_text_block(text: String) -> Value {
    json!({"type": "text", "text": text})
}

fn render_selected_memory_context(items: &[&MemorySelectionItemContextEntry]) -> Option<String> {
    let render_items = items
        .iter()
        .map(|item| RetrievedMemoryRenderItem {
            label: item.label.as_str(),
            detail: item.detail.as_str(),
        })
        .collect::<Vec<_>>();
    render_retrieved_memory_context(render_items.as_slice())
}

/// Extract the latest user message text from the conversation history.
fn latest_user_text(history: &[Message]) -> String {
    let raw = history
        .iter()
        .rev()
        .find(|msg| msg.role == "user")
        .and_then(extract_text);
    raw.unwrap_or_default()
}

fn extract_text(msg: &Message) -> Option<String> {
    msg.content.as_str().map(|s| s.to_string()).or_else(|| {
        msg.content.as_array().and_then(|items| {
            let joined: String = items
                .iter()
                .filter(|item| item.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            if joined.is_empty() {
                None
            } else {
                Some(joined)
            }
        })
    })
}
