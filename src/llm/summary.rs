use anyhow::{Result, bail};
use serde_json::Value;

use super::{ContentBlock, LlmExecutionMode, LlmResponse, Message};

/// Experiment selection; the default preserves auxiliary-model routing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SummaryStrategy {
    #[default]
    AuxiliaryModel,
    CachedMainModel,
}

/// Exact main-request context captured before sending, without accounting handles.
#[derive(Clone, Debug)]
pub struct SummaryPrefix {
    pub messages: Vec<Message>,
    pub tools: Vec<Value>,
    pub execution_mode: LlmExecutionMode,
}

impl SummaryPrefix {
    pub(crate) fn matches_history(&self, messages: &[Message]) -> bool {
        let leading = self
            .messages
            .iter()
            .take_while(|message| message.role == "system")
            .count();
        leading > 0
            && messages.len() >= self.messages.len() - leading
            && self.messages[leading..]
                .iter()
                .zip(messages)
                .all(|(old, new)| old == new)
    }

    pub(crate) fn messages_for_summary(
        &self,
        messages: &[Message],
        instruction: &str,
    ) -> Result<Vec<Message>> {
        let leading = self
            .messages
            .iter()
            .take_while(|message| message.role == "system")
            .count();
        // A trimmed suffix or a different projection is not a cache-sharing fork.
        // The caller can retry on the auxiliary route without mislabelling it.
        if !self.matches_history(messages) {
            bail!("summary input does not share the captured main-request prefix");
        }
        let mut request = self.messages[..leading].to_vec();
        request.extend_from_slice(messages);
        request.push(Message {
            role: "user".into(),
            content: Value::String(format!(
                "{instruction}\n\nReturn only the summary text. Do not call tools."
            )),
        });
        Ok(request)
    }
}

pub(super) fn summary_text(response: LlmResponse) -> Result<String> {
    let mut parts = Vec::new();
    for block in response.content {
        match block {
            ContentBlock::Text { text } => parts.push(text),
            ContentBlock::ToolUse { .. } => {
                bail!("summary response requested a tool; no tool was executed")
            }
            ContentBlock::ProviderMetadata { .. } => {}
        }
    }
    let text = parts.join("\n\n");
    if text.trim().is_empty() {
        bail!("summary response was empty")
    }
    Ok(text)
}
