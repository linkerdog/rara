use rara_tools::tool::ToolError;

use crate::tools::agent::AgentDefinition;

pub(super) fn agent_token_budget(
    definition: Option<&AgentDefinition>,
) -> Result<Option<u32>, ToolError> {
    let Some(raw) = definition.and_then(|definition| definition.token_budget) else {
        return Ok(None);
    };
    parse_agent_token_budget(raw)
}

pub(super) fn parse_agent_token_budget(raw: i64) -> Result<Option<u32>, ToolError> {
    if raw <= 0 {
        return Err(ToolError::InvalidInput(format!(
            "tokenBudget must be a positive token count; got {raw}"
        )));
    }
    let budget = u32::try_from(raw).map_err(|_| {
        ToolError::InvalidInput(format!("tokenBudget exceeds maximum u32 value; got {raw}"))
    })?;
    Ok(Some(budget))
}
