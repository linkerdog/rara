use serde_json::Value;

use super::Agent;
use crate::llm::{LlmTurnMetadata, Message, SummaryPrefix, SummaryStrategy};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToolSchemaPolicy {
    #[default]
    ModeFiltered,
    SessionStable,
}

/// Host-selected experiment controls; ordinary sessions keep existing behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheExperimentOptions {
    pub tool_schemas: ToolSchemaPolicy,
    pub summary: SummaryStrategy,
}

impl Agent {
    pub(crate) fn configure_cache_experiment(&mut self, options: CacheExperimentOptions) {
        self.stable_tool_schemas = match options.tool_schemas {
            ToolSchemaPolicy::ModeFiltered => None,
            ToolSchemaPolicy::SessionStable => Some(self.tool_manager.get_schemas()),
        };
        self.cache_experiment = options;
        self.summary_prefix = None;
    }

    pub(super) fn capture_summary_prefix(
        &mut self,
        messages: &[Message],
        tools: &[Value],
        metadata: &LlmTurnMetadata,
    ) {
        if self.cache_experiment.summary == SummaryStrategy::CachedMainModel {
            self.summary_prefix = Some(SummaryPrefix {
                messages: messages.to_vec(),
                tools: tools.to_vec(),
                execution_mode: metadata.execution_mode(),
            });
        }
    }
}
