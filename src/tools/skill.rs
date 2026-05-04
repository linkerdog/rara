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
    description = "Manage and invoke reusable skills. Skills are stored in SKILL.md files across home, repo, and cwd scopes. Higher-precedence scopes override lower-precedence skills with the same name. Use this to discover available skills and load their instructions.",
    input_schema = {
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "invoke"],
                "description": "list: discover available skills with their scopes, override status, and metadata. invoke: load a specific skill's full instructions by name."
            },
            "skill_name": {
                "type": "string",
                "description": "Name of the skill to invoke. Required when action is invoke. The skill name comes from frontmatter name field or the file/directory name."
            }
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
            "list" => {
                let scopes: Vec<String> = self
                    .skill_manager
                    .active_scopes()
                    .iter()
                    .map(|s| format!("{:?}", s).to_lowercase())
                    .collect();
                let skills: Vec<Value> = self
                    .skill_manager
                    .list_summaries()
                    .iter()
                    .map(|s| {
                        let overridden = self.skill_manager.is_overridden(&s.name);
                        json!({
                            "name": s.name,
                            "title": s.title,
                            "description": s.description,
                            "scope": format!("{:?}", s.scope).to_lowercase(),
                            "display_path": s.display_path,
                            "disable_model_invocation": s.disable_model_invocation,
                            "overridden": overridden,
                            "overridden_by": if overridden {
                                self.skill_manager.override_chain(&s.name)
                                    .iter()
                                    .map(|o| format!("{:?}", o.scope).to_lowercase())
                                    .collect::<Vec<_>>()
                            } else {
                                Vec::new()
                            }
                        })
                    })
                    .collect();
                Ok(json!({
                    "skills": skills,
                    "scopes": scopes,
                    "overrides": self.skill_manager.list_overrides(),
                    "load_warnings": &self.skill_manager.load_warnings,
                }))
            }
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
                let overridden_by: Vec<String> = self
                    .skill_manager
                    .override_chain(name)
                    .iter()
                    .map(|o| format!("{:?}", o.scope).to_lowercase())
                    .collect();
                Ok(json!({
                    "name": skill.name,
                    "title": skill.title,
                    "scope": format!("{:?}", skill.scope).to_lowercase(),
                    "display_path": skill.display_path,
                    "instructions": skill.prompt,
                    "disable_model_invocation": skill.disable_model_invocation,
                    "overridden": !overridden_by.is_empty(),
                    "overridden_by": overridden_by,
                }))
            }
            _ => Err(ToolError::InvalidInput("Invalid action".into())),
        }
    }
}
