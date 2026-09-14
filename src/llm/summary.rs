use anyhow::{Result, bail};
pub use rara_core::llm::backend::SummaryPrefix;
pub use rara_core::llm::contracts::SummaryStrategy;

use super::{ContentBlock, LlmResponse};

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
