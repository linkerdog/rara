use std::sync::{Arc, Weak};

use serde_json::Value;

use super::{Agent, AgentExecutionMode};
use crate::llm::{LlmBackend, LlmTurnMetadata, Message, SummaryPrefix, SummaryStrategy};

pub(super) struct CapturedSummaryPrefix {
    prefix: SummaryPrefix,
    backend: Weak<dyn LlmBackend>,
    execution_mode: AgentExecutionMode,
}

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
            self.summary_prefix = Some(CapturedSummaryPrefix {
                prefix: SummaryPrefix {
                    messages: messages.to_vec(),
                    tools: tools.to_vec(),
                    execution_mode: metadata.execution_mode(),
                },
                backend: Arc::downgrade(&self.llm_backend),
                execution_mode: self.execution_mode,
            });
        }
    }

    pub(super) fn cached_summary_prefix(&self, messages: &[Message]) -> Option<&SummaryPrefix> {
        let captured = self.summary_prefix.as_ref()?;
        // Endpoint/model labels do not identify a host-owned backend instance.
        (captured.backend.ptr_eq(&Arc::downgrade(&self.llm_backend))
            && captured.execution_mode == self.execution_mode
            && captured.prefix.tools == self.visible_tool_schemas()
            && captured.prefix.matches_history(messages))
        .then_some(&captured.prefix)
    }
}
