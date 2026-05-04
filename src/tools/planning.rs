use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};

pub const ENTER_PLAN_MODE_TOOL_NAME: &str = "enter_plan_mode";
pub const EXIT_PLAN_MODE_TOOL_NAME: &str = "exit_plan_mode";

pub struct EnterPlanModeTool;
pub struct ExitPlanModeTool;

#[tool_spec(
    name = ENTER_PLAN_MODE_TOOL_NAME,
    description = "Enter read-only planning mode when the task needs repository exploration and design before implementation",
    input_schema = {
        "type": "object",
        "properties": {},
    }
)]
#[async_trait]
impl Tool for EnterPlanModeTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({
            "status": "entered_plan_mode",
            "instructions": [
                "Inspect the repository with read-only tools.",
                "Return a normal final answer for research, review, or planning-advice tasks.",
                "Use a <proposed_plan> block only when you are requesting approval to implement a concrete plan.",
                "Call exit_plan_mode only after the same assistant message contains a complete <proposed_plan>...</proposed_plan> block.",
                "Use <request_user_input> only when a blocking decision needs user input.",
                "Use <continue_inspection/> only when another read-only inspection pass is required."
            ]
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::{EXIT_PLAN_MODE_TOOL_NAME, ExitPlanModeTool};
    use crate::tool::Tool;

    #[test]
    fn exit_plan_mode_schema_accepts_structured_proposed_plan() {
        let tool = ExitPlanModeTool;
        let schema = tool.input_schema();

        assert_eq!(tool.name(), EXIT_PLAN_MODE_TOOL_NAME);
        assert_eq!(schema["required"][0], "proposed_plan");
        assert_eq!(
            schema["properties"]["proposed_plan"]["required"],
            serde_json::json!(["summary", "steps", "validation"])
        );
        assert_eq!(
            schema["properties"]["proposed_plan"]["properties"]["steps"]["items"]["properties"]["status"]
                ["enum"],
            serde_json::json!(["pending", "in_progress", "completed"])
        );
    }
}

#[tool_spec(
    name = EXIT_PLAN_MODE_TOOL_NAME,
    description = "Submit a concrete implementation plan for structured user approval. Prefer passing the plan in the proposed_plan argument. If structured tool arguments are unavailable, this same assistant response must emit a complete <proposed_plan>...</proposed_plan> block before calling this tool.",
    input_schema = {
        "type": "object",
        "properties": {
            "proposed_plan": {
                "type": "object",
                "description": "Structured implementation plan to submit for approval.",
                "properties": {
                    "summary": {
                        "type": "string",
                        "description": "One concise sentence describing the implementation goal."
                    },
                    "steps": {
                        "type": "array",
                        "description": "Concrete implementation steps. Runtime treats only this array as executable plan state.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "step": {
                                    "type": "string",
                                    "description": "One concrete implementation step."
                                },
                                "status": {
                                    "type": "string",
                                    "enum": ["pending", "in_progress", "completed"],
                                    "description": "Current step status. Use pending for new plans unless a step is already complete."
                                }
                            },
                            "required": ["step", "status"],
                            "additionalProperties": false
                        }
                    },
                    "validation": {
                        "type": "array",
                        "description": "Focused tests or commands that should validate the implementation.",
                        "items": { "type": "string" }
                    }
                },
                "required": ["summary", "steps", "validation"],
                "additionalProperties": false
            }
        },
        "required": ["proposed_plan"],
        "additionalProperties": false,
    }
)]
#[async_trait]
impl Tool for ExitPlanModeTool {
    async fn call(&self, _input: Value) -> Result<Value, ToolError> {
        Ok(json!({
            "status": "exited_plan_mode",
            "instructions": [
                "Wait for the user's structured plan decision.",
                "If approved, continue in execute mode.",
                "If rejected or continued, refine the plan and call exit_plan_mode again only after emitting an updated complete <proposed_plan>...</proposed_plan> block."
            ]
        }))
    }
}
