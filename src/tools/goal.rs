use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};
use crate::tui::state::{GoalHandle, GoalStatus, RalphGoal};

pub const CREATE_GOAL_TOOL_NAME: &str = "create_goal";
pub const GET_GOAL_TOOL_NAME: &str = "get_goal";
pub const UPDATE_GOAL_TOOL_NAME: &str = "update_goal";

pub struct GetGoalTool {
    pub store: GoalHandle,
}

#[tool_spec(
    name = GET_GOAL_TOOL_NAME,
    description = "Get the current goal for this thread, including status, budgets, token and elapsed-time usage, and remaining token budget.",
    input_schema = {
        "type": "object",
        "properties": {},
    }
)]
#[async_trait]
impl Tool for GetGoalTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        let guard = self.store.read().unwrap();
        Ok(goal_tool_response(guard.as_ref(), false))
    }
}

pub struct CreateGoalTool {
    pub store: GoalHandle,
}

#[tool_spec(
    name = CREATE_GOAL_TOOL_NAME,
    description = "Create a goal only when explicitly requested by the user or system/developer instructions; do not infer goals from ordinary tasks.\nSet token_budget only when an explicit token budget is requested. Fails if a goal exists; use update_goal only for status.",
    input_schema = {
        "type": "object",
        "properties": {
            "objective": {
                "type": "string",
                "description": "Required. The concrete objective to start pursuing. This starts a new active goal only when no goal is currently defined; if a goal already exists, this tool fails."
            },
            "token_budget": {
                "type": "integer",
                "minimum": 1,
                "description": "Optional positive token budget for the new active goal."
            }
        },
        "required": ["objective"]
    }
)]
#[async_trait]
impl Tool for CreateGoalTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        if self.store.read().unwrap().is_some() {
            return Err(ToolError::InvalidInput(
                "cannot create a new goal because this thread already has a goal; use update_goal only when the existing goal is complete".into(),
            ));
        }

        let objective = input["objective"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("objective must be a string".into()))?
            .to_string();
        if objective.trim().is_empty() {
            return Err(ToolError::InvalidInput(
                "objective must not be empty".into(),
            ));
        }
        let token_budget = match input["token_budget"].as_u64() {
            None => None,
            Some(0) => {
                return Err(ToolError::InvalidInput(
                    "token_budget must be positive".into(),
                ));
            }
            Some(v) if v > u32::MAX as u64 => {
                return Err(ToolError::InvalidInput(format!(
                    "token_budget {v} exceeds maximum u32 value"
                )));
            }
            Some(v) => Some(v as u32),
        };

        let goal = RalphGoal::new(objective, token_budget);
        let response = goal_tool_response(Some(&goal), false);
        *self.store.write().unwrap() = Some(goal);

        Ok(response)
    }
}

pub struct UpdateGoalTool {
    pub store: GoalHandle,
}

#[tool_spec(
    name = UPDATE_GOAL_TOOL_NAME,
    description = "Update the existing goal.\nUse this tool only to mark the goal achieved.\nSet status to `complete` only when the objective has actually been achieved and no required work remains.\nDo not mark a goal complete merely because its budget is nearly exhausted or because you are stopping work.\nYou cannot use this tool to pause, resume, or budget-limit a goal; those status changes are controlled by the user or system.\nWhen marking a budgeted goal achieved with status `complete`, report the final token usage from the tool result to the user.",
    input_schema = {
        "type": "object",
        "properties": {
            "status": {
                "type": "string",
                "enum": ["complete"],
                "description": "Required. Set to complete only when the objective is achieved and no required work remains."
            }
        },
        "required": ["status"]
    }
)]
#[async_trait]
impl Tool for UpdateGoalTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let new_status = input["status"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("status must be a string".into()))?;

        match new_status {
            "complete" => {}
            other => {
                return Err(ToolError::InvalidInput(format!(
                    "update_goal can only mark the existing goal complete; pause, resume, and budget-limited status changes are controlled by the user or system (got '{other}')"
                )));
            }
        }

        let mut guard = self.store.write().unwrap();
        let goal = guard
            .as_mut()
            .ok_or_else(|| ToolError::InvalidInput("No active goal to update.".into()))?;

        goal.status = GoalStatus::Complete;

        Ok(goal_tool_response(Some(goal), true))
    }
}

fn goal_status_str(status: GoalStatus) -> &'static str {
    match status {
        GoalStatus::Pursuing => "active",
        GoalStatus::Paused => "paused",
        GoalStatus::Complete => "complete",
        GoalStatus::BudgetLimited => "budget_limited",
    }
}

fn goal_tool_response(goal: Option<&RalphGoal>, include_completion_report: bool) -> Value {
    let remaining_tokens = goal.and_then(RalphGoal::remaining_tokens);
    let completion_budget_report = if include_completion_report {
        goal.and_then(completion_budget_report)
    } else {
        None
    };
    let goal = goal.map(|g| {
        json!({
            "objective": g.objective.as_str(),
            "status": goal_status_str(g.status),
            "token_budget": g.token_budget,
            "tokens_used": g.tokens_used,
            "turns_completed": g.turns_completed,
            "time_used_seconds": g.time_used_seconds(),
        })
    });
    json!({
        "goal": goal,
        "remainingTokens": remaining_tokens,
        "completionBudgetReport": completion_budget_report,
    })
}

fn completion_budget_report(goal: &RalphGoal) -> Option<String> {
    if goal.status != GoalStatus::Complete {
        return None;
    }
    let mut parts = Vec::new();
    if let Some(budget) = goal.token_budget {
        parts.push(format!("tokens used: {} of {budget}", goal.tokens_used));
    }
    let time_used_seconds = goal.time_used_seconds();
    if time_used_seconds > 0 {
        parts.push(format!("time used: {time_used_seconds} seconds"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(format!(
            "Goal achieved. Report final budget usage to the user: {}.",
            parts.join("; ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::tool::Tool;

    fn goal_handle() -> GoalHandle {
        Arc::new(std::sync::RwLock::new(None))
    }

    fn block<F: std::future::Future>(f: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }

    #[test]
    fn create_and_read_goal_roundtrip() {
        let store = goal_handle();
        let create = CreateGoalTool {
            store: store.clone(),
        };
        let get = GetGoalTool {
            store: store.clone(),
        };

        let result = block(
            create.call(serde_json::json!({"objective": "Fix the build", "token_budget": 50000})),
        )
        .unwrap();
        assert_eq!(result["goal"]["objective"], "Fix the build");
        assert_eq!(result["goal"]["status"], "active");
        assert_eq!(result["goal"]["token_budget"], 50000);
        assert_eq!(result["goal"]["turns_completed"], 0);
        assert_eq!(result["remainingTokens"], 50000);
        assert_eq!(result["completionBudgetReport"], serde_json::Value::Null);

        let state = block(get.call(serde_json::json!({}))).unwrap();
        assert_eq!(state["goal"]["objective"], "Fix the build");
        assert_eq!(state["goal"]["status"], "active");
        assert_eq!(state["goal"]["token_budget"], 50000);
        assert_eq!(state["goal"]["tokens_used"], 0);
        assert_eq!(state["remainingTokens"], 50000);
    }

    #[test]
    fn create_goal_fails_when_goal_exists() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal::new("existing".into(), None));

        let create = CreateGoalTool {
            store: store.clone(),
        };
        let err = block(create.call(serde_json::json!({"objective": "new"}))).unwrap_err();
        assert!(err.to_string().contains("already has a goal"));
    }

    #[test]
    fn create_goal_rejects_empty_objective() {
        let store = goal_handle();
        let create = CreateGoalTool { store };
        let err = block(create.call(serde_json::json!({"objective": ""}))).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn create_goal_rejects_whitespace_only_objective() {
        let store = goal_handle();
        let create = CreateGoalTool { store };
        let err = block(create.call(serde_json::json!({"objective": "   "}))).unwrap_err();
        assert!(err.to_string().contains("must not be empty"));
    }

    #[test]
    fn create_goal_rejects_oversized_token_budget() {
        let store = goal_handle();
        let create = CreateGoalTool { store };
        let err = block(
            create.call(serde_json::json!({"objective": "x", "token_budget": 5_000_000_000u64})),
        )
        .unwrap_err();
        assert!(err.to_string().contains("exceeds maximum u32 value"));
    }

    #[test]
    fn create_goal_rejects_zero_token_budget() {
        let store = goal_handle();
        let create = CreateGoalTool { store };
        let err = block(create.call(serde_json::json!({"objective": "x", "token_budget": 0})))
            .unwrap_err();
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn update_goal_schema_only_exposes_complete_status() {
        let store = goal_handle();
        let update = UpdateGoalTool { store };
        let schema = update.input_schema();

        assert_eq!(
            schema["properties"]["status"]["enum"],
            serde_json::json!(["complete"])
        );
    }

    #[test]
    fn update_goal_marks_complete_and_reports_budget() {
        let store = goal_handle();
        let mut goal = RalphGoal::new("test".into(), Some(2000));
        goal.tokens_used = 1000;
        goal.turns_completed = 3;
        goal.created_at_epoch_seconds =
            crate::tui::state::current_unix_timestamp_secs().saturating_sub(75);
        *store.write().unwrap() = Some(goal);

        let update = UpdateGoalTool {
            store: store.clone(),
        };
        let result = block(update.call(serde_json::json!({"status": "complete"}))).unwrap();
        assert_eq!(result["goal"]["status"], "complete");
        assert_eq!(result["goal"]["objective"], "test");
        assert_eq!(result["goal"]["tokens_used"], 1000);
        assert_eq!(result["goal"]["turns_completed"], 3);
        assert_eq!(result["remainingTokens"], 1000);
        assert!(
            result["completionBudgetReport"]
                .as_str()
                .expect("completion report")
                .contains("tokens used: 1000 of 2000")
        );
    }

    #[test]
    fn update_goal_rejects_invalid_status() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal::new("test".into(), None));

        let update = UpdateGoalTool { store };
        let err = block(update.call(serde_json::json!({"status": "pursuing"}))).unwrap_err();
        assert!(
            err.to_string()
                .contains("only mark the existing goal complete")
        );
    }

    #[test]
    fn update_goal_fails_without_existing_goal() {
        let store = goal_handle();
        let update = UpdateGoalTool { store };
        let err = block(update.call(serde_json::json!({"status": "complete"}))).unwrap_err();
        assert!(err.to_string().contains("No active goal"));
    }

    #[test]
    fn get_goal_returns_nulls_when_empty() {
        let store = goal_handle();
        let get = GetGoalTool { store };
        let result = block(get.call(serde_json::json!({}))).unwrap();
        assert_eq!(result["goal"], serde_json::Value::Null);
        assert_eq!(result["remainingTokens"], serde_json::Value::Null);
        assert_eq!(result["completionBudgetReport"], serde_json::Value::Null);
    }
}
