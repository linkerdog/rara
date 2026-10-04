use super::ContextAssembler;
use crate::prompt::{BasePromptKind, EffectivePrompt, PromptMode, PromptRuntimeConfig};
use crate::workspace::WorkspaceMemory;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceMemoryAvailability {
    Available,
    Missing,
}

impl WorkspaceMemoryAvailability {
    pub(crate) fn read(workspace: &WorkspaceMemory) -> Self {
        if workspace.has_memory_file_cached() {
            Self::Available
        } else {
            Self::Missing
        }
    }
}

/// Filesystem inputs for display assembly. Model requests always load their own
/// current inputs; this cache never changes provider prompts or persisted history.
pub(crate) struct RuntimeContextFiles {
    pub cwd: String,
    pub branch: String,
    pub effective_prompt: EffectivePrompt,
    pub memory: WorkspaceMemoryAvailability,
}

impl RuntimeContextFiles {
    pub(crate) fn load(
        workspace: &WorkspaceMemory,
        config: &PromptRuntimeConfig,
        mode: PromptMode,
    ) -> Self {
        let effective_prompt = ContextAssembler::new(workspace, config).effective_prompt(mode);
        let (cwd, branch) = workspace.get_env_info();
        Self {
            cwd,
            branch,
            effective_prompt,
            memory: WorkspaceMemoryAvailability::read(workspace),
        }
    }

    pub(crate) fn pending(workspace: &WorkspaceMemory, config: &PromptRuntimeConfig) -> Self {
        Self {
            cwd: workspace.root.display().to_string(),
            branch: String::new(),
            effective_prompt: EffectivePrompt {
                text: String::new(),
                base_prompt_kind: if config.system_prompt.is_some() {
                    BasePromptKind::Custom
                } else {
                    BasePromptKind::Default
                },
                section_keys: Vec::new(),
                sources: Vec::new(),
            },
            memory: WorkspaceMemoryAvailability::Missing,
        }
    }
}
