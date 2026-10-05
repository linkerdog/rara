use std::sync::Arc;

use crate::llm::LlmBackend;
use crate::runtime_control::{
    McpSourceControlRequest, PromptSourceControlRequest, RuntimeProvenance,
    SkillSourceControlRequest,
};

pub(super) enum NativeControl {
    McpSource {
        request: McpSourceControlRequest,
        provenance: RuntimeProvenance,
    },
    ReplaceBackend {
        backend: Arc<dyn LlmBackend>,
    },
    SetMaxTurns {
        max_turns: usize,
    },
    DisableTools,
    DisableExtensionExecution,
    SetFullAccess {
        enabled: bool,
    },
    PromptSource {
        request: PromptSourceControlRequest,
        provenance: RuntimeProvenance,
    },
    SkillSource {
        request: SkillSourceControlRequest,
        provenance: RuntimeProvenance,
    },
}
