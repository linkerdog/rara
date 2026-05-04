use std::sync::Arc;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::skill::SkillManager;
use crate::tool::{Tool, ToolError};

pub struct SkillTool {
    pub skill_manager: Arc<SkillManager>,
}
#[tool_spec(
    name = "skill",
    description = "Manage and invoke reusable skills",
    input_schema = {
        "type": "object",
        "properties": {
            "action": { "type": "string", "enum": ["list", "invoke"] },
            "skill_name": { "type": "string" }
        },
        "required": ["action"]
    }
)]
#[async_trait]
impl Tool for SkillTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let action = i["action"]
            .as_str()
            .ok_or(ToolError::InvalidInput("action".into()))?;
        match action {
            "list" => Ok(json!({
                "skills": self.skill_manager.list_summaries(),
                "overrides": self.skill_manager.list_overrides(),
                "load_warnings": &self.skill_manager.load_warnings,
            })),
            "invoke" => {
                let name = i["skill_name"]
                    .as_str()
                    .ok_or(ToolError::InvalidInput("name".into()))?;
                let skill =
                    self.skill_manager
                        .get_skill(name)
                        .ok_or(ToolError::ExecutionFailed(format!(
                            "Skill not found: {name}"
                        )))?;
                Ok(json!({
                    "name": skill.name,
                    "title": skill.title,
                    "scope": skill.scope,
                    "display_path": skill.display_path,
                    "instructions": skill.prompt,
                }))
            }
            _ => Err(ToolError::InvalidInput("Invalid action".into())),
        }
    }
}
