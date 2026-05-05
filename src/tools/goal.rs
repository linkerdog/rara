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
    description = "Retrieve the current active goal state (objective, status, budget, usage).",
    input_schema = {
        "type": "object",
        "properties": {},
    }
)]
#[async_trait]
impl Tool for GetGoalTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        let guard = self.store.read().unwrap();
        match guard.as_ref() {
            None => Ok(json!({
                "objective": null,
                "status": null,
                "token_budget": null,
                "tokens_used": 0,
                "turns_completed": 0,
            })),
            Some(g) => {
                let status = match g.status {
                    GoalStatus::Pursuing => "pursuing",
                    GoalStatus::Paused => "paused",
                    GoalStatus::Achieved => "achieved",
                    GoalStatus::Unmet => "unmet",
                    GoalStatus::BudgetLimited => "budget_limited",
                };
                Ok(json!({
                    "objective": g.objective,
                    "status": status,
                    "token_budget": g.token_budget,
                    "tokens_used": g.tokens_used,
                    "turns_completed": g.turns_completed,
                }))
            }
        }
    }
}

pub struct CreateGoalTool {
    pub store: GoalHandle,
}

#[tool_spec(
    name = CREATE_GOAL_TOOL_NAME,
    description = "Create a new goal only when no goal exists. Goals define a long-running objective the agent works toward across turns. Include an optional token_budget to limit total token consumption.",
    input_schema = {
        "type": "object",
        "properties": {
            "objective": {
                "type": "string",
                "description": "The goal objective text. Keep it focused and actionable."
            },
            "token_budget": {
                "type": "integer",
                "description": "Optional maximum input tokens for this goal. Omit for unlimited."
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
                "A goal already exists. Use update_goal to modify it, or clear it first.".into(),
            ));
        }

        let objective = input["objective"]
            .as_str()
            .ok_or_else(|| ToolError::InvalidInput("objective must be a string".into()))?
            .to_string();
        let token_budget = input["token_budget"].as_u64().map(|v| v as u32);

        let goal = RalphGoal {
            objective: objective.clone(),
            status: GoalStatus::Pursuing,
            token_budget,
            tokens_used: 0,
            turns_completed: 0,
        };
        *self.store.write().unwrap() = Some(goal);

        Ok(json!({
            "objective": objective,
            "status": "pursuing",
            "token_budget": token_budget,
            "tokens_used": 0,
            "turns_completed": 0,
        }))
    }
}

pub struct UpdateGoalTool {
    pub store: GoalHandle,
}

#[tool_spec(
    name = UPDATE_GOAL_TOOL_NAME,
    description = "Update the current goal status. Use this to mark a goal as achieved or unmet. Do not set pursuing/paused/budget_limited — those are managed by the runtime.",
    input_schema = {
        "type": "object",
        "properties": {
            "status": {
                "type": "string",
                "enum": ["achieved", "unmet"],
                "description": "New goal status. Only achieved or unmet — the runtime owns the other states."
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

        let new_status = match new_status {
            "achieved" => GoalStatus::Achieved,
            "unmet" => GoalStatus::Unmet,
            other => {
                return Err(ToolError::InvalidInput(format!(
                    "Invalid status '{other}'. The model may only set achieved or unmet. \
                     Use the runtime to set pursuing, paused, or budget_limited."
                )));
            }
        };

        let mut guard = self.store.write().unwrap();
        let goal = guard
            .as_mut()
            .ok_or_else(|| ToolError::InvalidInput("No active goal to update.".into()))?;

        goal.status = new_status;

        let status_str = match new_status {
            GoalStatus::Achieved => "achieved",
            GoalStatus::Unmet => "unmet",
            _ => unreachable!(),
        };

        Ok(json!({
            "objective": goal.objective,
            "status": status_str,
            "token_budget": goal.token_budget,
            "tokens_used": goal.tokens_used,
            "turns_completed": goal.turns_completed,
        }))
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

    #[test]
    fn goal_tools_expose_correct_names() {
        let store = goal_handle();
        assert_eq!(
            GetGoalTool {
                store: store.clone()
            }
            .name(),
            GET_GOAL_TOOL_NAME
        );
        assert_eq!(
            CreateGoalTool {
                store: store.clone()
            }
            .name(),
            CREATE_GOAL_TOOL_NAME
        );
        assert_eq!(
            UpdateGoalTool {
                store: store.clone()
            }
            .name(),
            UPDATE_GOAL_TOOL_NAME
        );
    }

    #[test]
    fn create_goal_schema_requires_objective() {
        let store = goal_handle();
        let tool = CreateGoalTool { store };
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "objective");
    }

    #[test]
    fn update_goal_schema_requires_status() {
        let store = goal_handle();
        let tool = UpdateGoalTool { store };
        let schema = tool.input_schema();
        assert_eq!(schema["required"][0], "status");
    }
}
