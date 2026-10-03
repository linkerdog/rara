use std::sync::Arc;

use crate::llm::LlmBackend;
use crate::runtime_control::{
    PromptSourceControlRequest, RuntimeProvenance, SkillSourceControlRequest,
};

pub(super) enum NativeControl {
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
