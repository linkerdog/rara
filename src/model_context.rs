use rara_core::llm::types::Message;
use serde_json::{Value, json};

pub(crate) const MODEL_CONTEXT_BLOCK_TYPE: &str = "rara_model_context";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModelContextKind {
    Environment,
    ExecutionMode,
    ProtocolPromptSources,
    RetrievedMemory,
}

impl ModelContextKind {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Environment => "environment",
            Self::ExecutionMode => "execution_mode",
            Self::ProtocolPromptSources => "protocol_prompt_sources",
            Self::RetrievedMemory => "retrieved_memory",
        }
    }

    const fn sort_key(self) -> u8 {
        match self {
            Self::Environment => 0,
            Self::ExecutionMode => 1,
            Self::ProtocolPromptSources => 2,
            Self::RetrievedMemory => 3,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ModelContextFragment {
    pub(crate) kind: ModelContextKind,
    pub(crate) text: String,
}

impl ModelContextFragment {
    pub(crate) fn new(kind: ModelContextKind, text: impl Into<String>) -> Self {
        Self {
            kind,
            text: text.into(),
        }
    }
}

pub(crate) fn model_context_text(block: &Value) -> Option<&str> {
    (block.get("type").and_then(Value::as_str) == Some(MODEL_CONTEXT_BLOCK_TYPE))
        .then(|| block.get("text").and_then(Value::as_str))
        .flatten()
}

pub(crate) fn latest_model_context_text(
    messages: &[Message],
    kind: ModelContextKind,
) -> Option<&str> {
    messages.iter().rev().find_map(|message| {
        message.content.as_array()?.iter().rev().find_map(|block| {
            (model_context_kind(block) == Some(kind))
                .then(|| model_context_text(block))
                .flatten()
        })
    })
}

pub(crate) fn upsert_latest_user_model_context(
    messages: &mut [Message],
    fragments: &[ModelContextFragment],
) -> bool {
    if fragments.is_empty() {
        return false;
    }
    let Some(message) = messages
        .iter_mut()
        .rfind(|message| message.role == "user" && is_user_text_request(message))
    else {
        return false;
    };

    let previous = message.content.clone();
    let mut content = match std::mem::take(&mut message.content) {
        Value::Array(items) => items,
        Value::String(text) => vec![json!({"type": "text", "text": text})],
        other => vec![json!({"type": "text", "text": other.to_string()})],
    };
    content.retain(|block| {
        model_context_kind(block)
            .is_none_or(|kind| !fragments.iter().any(|fragment| fragment.kind == kind))
    });
    content.splice(
        0..0,
        fragments
            .iter()
            .map(|fragment| model_context_block(fragment.kind, fragment.text.as_str())),
    );
    content
        .sort_by_key(|block| model_context_kind(block).map_or(u8::MAX, ModelContextKind::sort_key));
    message.content = Value::Array(content);
    message.content != previous
}

fn model_context_block(kind: ModelContextKind, text: &str) -> Value {
    json!({
        "type": MODEL_CONTEXT_BLOCK_TYPE,
        "kind": kind.as_str(),
        "text": text,
    })
}

pub(crate) fn model_context_kind(block: &Value) -> Option<ModelContextKind> {
    if block.get("type").and_then(Value::as_str) != Some(MODEL_CONTEXT_BLOCK_TYPE) {
        return None;
    }
    match block.get("kind").and_then(Value::as_str)? {
        "environment" => Some(ModelContextKind::Environment),
        "execution_mode" => Some(ModelContextKind::ExecutionMode),
        "protocol_prompt_sources" => Some(ModelContextKind::ProtocolPromptSources),
        "retrieved_memory" => Some(ModelContextKind::RetrievedMemory),
        _ => None,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_context_is_upserted_before_user_text() {
        let mut messages = vec![Message {
            role: "user".to_string(),
            content: json!([{"type": "text", "text": "inspect the cache"}]),
        }];
        let fragments = vec![
            ModelContextFragment::new(ModelContextKind::Environment, "workspace A"),
            ModelContextFragment::new(ModelContextKind::ExecutionMode, "execute"),
        ];

        assert!(upsert_latest_user_model_context(&mut messages, &fragments));
        let blocks = messages[0].content.as_array().expect("content blocks");
        assert_eq!(blocks[0]["kind"], "environment");
        assert_eq!(blocks[1]["kind"], "execution_mode");
        assert_eq!(blocks[2]["text"], "inspect the cache");
        assert_eq!(
            latest_model_context_text(&messages, ModelContextKind::Environment),
            Some("workspace A")
        );

        assert!(!upsert_latest_user_model_context(&mut messages, &fragments));
        assert!(upsert_latest_user_model_context(
            &mut messages,
            &[ModelContextFragment::new(
                ModelContextKind::RetrievedMemory,
                "memory"
            )]
        ));
        let blocks = messages[0].content.as_array().expect("content blocks");
        assert_eq!(blocks[0]["kind"], "environment");
        assert_eq!(blocks[1]["kind"], "execution_mode");
        assert_eq!(blocks[2]["kind"], "retrieved_memory");
        assert_eq!(blocks[3]["text"], "inspect the cache");
        assert_eq!(
            messages[0]
                .content
                .as_array()
                .expect("content blocks")
                .len(),
            4
        );
    }
}
