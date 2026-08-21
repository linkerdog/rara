use serde_json::Value;

use super::{Agent, Message};
use crate::context::{
    MemoryRetrievalOrchestrator, MemorySelectionItemContextEntry, RetrievedMemoryRenderItem,
    SharedRuntimeContext, is_retrieved_memory_kind, render_retrieved_memory_context,
};
use crate::model_context::{
    ModelContextFragment, ModelContextKind, latest_model_context_text,
    upsert_latest_user_model_context,
};
use crate::prompt;

const CLEARED_PROTOCOL_PROMPT_SOURCES: &str =
    "## Protocol Prompt Sources\n\nNo protocol prompt sources are active for this turn.";

impl Agent {
    pub(super) async fn refresh_memory_retrieval_candidates(&mut self) {
        self.retrieved_memory_candidates = MemoryRetrievalOrchestrator::new(
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

    pub(super) fn persist_model_context_for_latest_user_message(&mut self) -> bool {
        let turn_context = prompt::build_turn_prompt_context(
            &self.workspace,
            &self.prompt_config,
            self.prompt_mode(),
        );
        let mut fragments = Vec::new();

        push_changed_context(
            &self.history,
            &mut fragments,
            ModelContextKind::Environment,
            turn_context.environment,
        );
        push_changed_context(
            &self.history,
            &mut fragments,
            ModelContextKind::ExecutionMode,
            turn_context.execution_mode,
        );
        match turn_context.protocol_prompt_sources {
            Some(protocol_sources) => push_changed_context(
                &self.history,
                &mut fragments,
                ModelContextKind::ProtocolPromptSources,
                protocol_sources,
            ),
            None if latest_model_context_text(
                &self.history,
                ModelContextKind::ProtocolPromptSources,
            )
            .is_some_and(|text| text != CLEARED_PROTOCOL_PROMPT_SOURCES) =>
            {
                fragments.push(ModelContextFragment::new(
                    ModelContextKind::ProtocolPromptSources,
                    CLEARED_PROTOCOL_PROMPT_SOURCES,
                ));
            }
            None => {}
        }
        let mut changed = upsert_latest_user_model_context(&mut self.history, fragments.as_slice());

        let assembled = self.assemble_turn_context();
        if let Some(memory_context) = Agent::selected_memory_context_text(&assembled.runtime) {
            changed |= upsert_latest_user_model_context(
                &mut self.history,
                &[ModelContextFragment::new(
                    ModelContextKind::RetrievedMemory,
                    memory_context,
                )],
            );
        }

        changed
    }
}

fn push_changed_context(
    history: &[Message],
    fragments: &mut Vec<ModelContextFragment>,
    kind: ModelContextKind,
    text: String,
) {
    if latest_model_context_text(history, kind) != Some(text.as_str()) {
        fragments.push(ModelContextFragment::new(kind, text));
    }
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
