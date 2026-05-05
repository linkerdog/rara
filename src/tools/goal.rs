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
                "A goal already exists. Use update_goal to mark it achieved or unmet, or clear it first to set a new one.".into(),
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
        let token_budget = match input.get("token_budget") {
            None | Some(serde_json::Value::Null) => None,
            Some(v) => match v.as_u64() {
                Some(n) if n > u32::MAX as u64 => {
                    return Err(ToolError::InvalidInput(format!(
                        "token_budget {n} exceeds maximum u32 value"
                    )));
                }
                Some(n) => Some(n as u32),
                None => {
                    return Err(ToolError::InvalidInput(
                        "token_budget must be a non-negative integer".into(),
                    ));
                }
            },
        };

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
        assert_eq!(result["objective"], "Fix the build");
        assert_eq!(result["status"], "pursuing");
        assert_eq!(result["token_budget"], 50000);
        assert_eq!(result["turns_completed"], 0);

        let state = block(get.call(serde_json::json!({}))).unwrap();
        assert_eq!(state["objective"], "Fix the build");
        assert_eq!(state["status"], "pursuing");
        assert_eq!(state["token_budget"], 50000);
        assert_eq!(state["tokens_used"], 0);
    }

    #[test]
    fn create_goal_fails_when_goal_exists() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal {
            objective: "existing".into(),
            status: GoalStatus::Pursuing,
            token_budget: None,
            tokens_used: 0,
            turns_completed: 0,
        });

        let create = CreateGoalTool {
            store: store.clone(),
        };
        let err = block(create.call(serde_json::json!({"objective": "new"}))).unwrap_err();
        assert!(err.to_string().contains("already exists"));
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
    fn update_goal_mark_achieved() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal {
            objective: "test".into(),
            status: GoalStatus::Pursuing,
            token_budget: None,
            tokens_used: 1000,
            turns_completed: 3,
        });

        let update = UpdateGoalTool {
            store: store.clone(),
        };
        let result = block(update.call(serde_json::json!({"status": "achieved"}))).unwrap();
        assert_eq!(result["status"], "achieved");
        assert_eq!(result["objective"], "test");
        assert_eq!(result["tokens_used"], 1000);
        assert_eq!(result["turns_completed"], 3);
    }

    #[test]
    fn update_goal_mark_unmet() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal {
            objective: "blocked task".into(),
            status: GoalStatus::Pursuing,
            token_budget: None,
            tokens_used: 0,
            turns_completed: 0,
        });

        let update = UpdateGoalTool { store };
        let result = block(update.call(serde_json::json!({"status": "unmet"}))).unwrap();
        assert_eq!(result["status"], "unmet");
    }

    #[test]
    fn update_goal_rejects_invalid_status() {
        let store = goal_handle();
        *store.write().unwrap() = Some(RalphGoal {
            objective: "test".into(),
            status: GoalStatus::Pursuing,
            token_budget: None,
            tokens_used: 0,
            turns_completed: 0,
        });

        let update = UpdateGoalTool { store };
        let err = block(update.call(serde_json::json!({"status": "pursuing"}))).unwrap_err();
        assert!(err.to_string().contains("Invalid status"));
    }

    #[test]
    fn update_goal_fails_without_existing_goal() {
        let store = goal_handle();
        let update = UpdateGoalTool { store };
        let err = block(update.call(serde_json::json!({"status": "achieved"}))).unwrap_err();
        assert!(err.to_string().contains("No active goal"));
    }

    #[test]
    fn get_goal_returns_nulls_when_empty() {
        let store = goal_handle();
        let get = GetGoalTool { store };
        let result = block(get.call(serde_json::json!({}))).unwrap();
        assert_eq!(result["objective"], serde_json::Value::Null);
        assert_eq!(result["status"], serde_json::Value::Null);
        assert_eq!(result["tokens_used"], 0);
    }
}
