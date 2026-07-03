use rara_tools::tool::ToolError;

use crate::agent::BashApprovalMode;
use crate::tools::agent::AgentDefinition;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum AgentPermissionMode {
    Default,
    AcceptEdits,
    Auto,
    Plan,
    BypassPermissions,
}

impl AgentPermissionMode {
    pub(super) fn requires_plan_mode(self) -> bool {
        matches!(self, Self::Plan)
    }

    pub(super) fn bash_approval_mode(self, plan_required: bool) -> BashApprovalMode {
        if plan_required || matches!(self, Self::AcceptEdits) {
            BashApprovalMode::Suggestion
        } else {
            BashApprovalMode::Always
        }
    }

    pub(super) fn full_access_mode(self, plan_required: bool) -> bool {
        !plan_required && matches!(self, Self::BypassPermissions)
    }
}

pub(super) fn agent_permission_mode(
    definition: Option<&AgentDefinition>,
) -> Result<AgentPermissionMode, ToolError> {
    let Some(raw) = definition.and_then(|definition| definition.permission_mode.as_deref()) else {
        return Ok(AgentPermissionMode::Default);
    };
    parse_agent_permission_mode(raw)
}

pub(super) fn parse_agent_permission_mode(raw: &str) -> Result<AgentPermissionMode, ToolError> {
    let trimmed = raw.trim();
    let normalized = trimmed.to_ascii_lowercase();
    match normalized.as_str() {
        "" | "default" => Ok(AgentPermissionMode::Default),
        "acceptedits" | "accept-edits" | "accept_edits" => Ok(AgentPermissionMode::AcceptEdits),
        "auto" => Ok(AgentPermissionMode::Auto),
        "plan" | "readonly" | "read-only" | "read_only" => Ok(AgentPermissionMode::Plan),
        "bypasspermissions" | "bypass-permissions" | "bypass_permissions" | "fullaccess"
        | "full-access" | "full_access" => Ok(AgentPermissionMode::BypassPermissions),
        _ => Err(ToolError::InvalidInput(format!(
            "permissionMode must be one of default, acceptEdits, accept-edits, auto, plan, readOnly, read-only, bypassPermissions, bypass-permissions, fullAccess, or full-access; got {trimmed}"
        ))),
    }
}
