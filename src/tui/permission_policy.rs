use crate::agent::{AgentExecutionMode, BashApprovalMode};
use crate::tui::state::PermissionMode;

pub(crate) struct PermissionPreset {
    pub(crate) mode: PermissionMode,
    pub(crate) title: &'static str,
    pub(crate) description: &'static str,
    pub(crate) execution: AgentExecutionMode,
    pub(crate) approval: BashApprovalMode,
    pub(crate) network: bool,
    pub(crate) full_access: bool,
}

pub(crate) const PERMISSION_PRESETS: [PermissionPreset; 4] = [
    PermissionPreset {
        mode: PermissionMode::Auto,
        title: "Auto",
        description: "Allow edits and ordinary commands. Escalation checks remain. Sandbox network off.",
        execution: AgentExecutionMode::Execute,
        approval: BashApprovalMode::Always,
        network: false,
        full_access: false,
    },
    PermissionPreset {
        mode: PermissionMode::AcceptEdits,
        title: "Accept edits",
        description: "Allow edits; ask for shell commands except reads and approved prefixes. Sandbox network off.",
        execution: AgentExecutionMode::Execute,
        approval: BashApprovalMode::Suggestion,
        network: false,
        full_access: false,
    },
    PermissionPreset {
        mode: PermissionMode::ReadOnly,
        title: "Read only (plan)",
        description: "Plan and inspect; no file edits or mutating shell commands. Sandbox network off.",
        execution: AgentExecutionMode::Plan,
        approval: BashApprovalMode::Suggestion,
        network: false,
        full_access: false,
    },
    PermissionPreset {
        mode: PermissionMode::FullAccess,
        title: "Full access (always allow)",
        description: "Allow edits, commands, and sandbox network. Bypass local approval and classifier checks.",
        execution: AgentExecutionMode::Execute,
        approval: BashApprovalMode::Always,
        network: true,
        full_access: true,
    },
];

pub(crate) fn permission_preset(mode: PermissionMode) -> Option<&'static PermissionPreset> {
    PERMISSION_PRESETS.iter().find(|preset| preset.mode == mode)
}
