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

#[cfg(test)]
mod tests {
    use rara_skills::SkillManager;
    use serde_json::json;

    use super::*;

    #[tokio::test]
    async fn list_returns_scopes_and_skills() {
        let mut manager = SkillManager::new();
        manager.load_warnings = vec!["test warning".into()];

        let tool = SkillTool {
            skill_manager: Arc::new(manager),
        };

        let result = tool.call(json!({"action": "list"})).await.expect("list");
        let skills = result["skills"].as_array().expect("skills array");
        let scopes = result["scopes"].as_array().expect("scopes array");
        assert!(skills.is_empty());
        assert!(scopes.is_empty());
        assert_eq!(result["load_warnings"][0].as_str(), Some("test warning"));
    }

    #[tokio::test]
    async fn invoke_returns_overridden_by_when_present() {
        let mut manager = SkillManager::new();
        // Simulate a loaded skill by directly inserting into the skills map.
        manager.skills.insert(
            "test-skill".into(),
            rara_skills::Skill {
                name: "test-skill".into(),
                title: Some("Test Skill".into()),
                description: "A test".into(),
                prompt: "# Test\nbody".into(),
                display_path: "test-skill/SKILL.md".into(),
                scope: rara_skills::SkillScope::Cwd,
                disable_model_invocation: false,
            },
        );

        let tool = SkillTool {
            skill_manager: Arc::new(manager),
        };

        let result = tool
            .call(json!({"action": "invoke", "skill_name": "test-skill"}))
            .await
            .expect("invoke");

        assert_eq!(result["name"].as_str(), Some("test-skill"));
        assert_eq!(result["title"].as_str(), Some("Test Skill"));
        assert_eq!(result["scope"].as_str(), Some("cwd"));
        assert_eq!(result["instructions"].as_str(), Some("# Test\nbody"));
        assert_eq!(result["overridden"].as_bool(), Some(false));
        assert!(result["overridden_by"].as_array().unwrap().is_empty());
    }

    #[tokio::test]
    async fn invoke_missing_skill_returns_error() {
        let manager = SkillManager::new();
        let tool = SkillTool {
            skill_manager: Arc::new(manager),
        };

        let err = tool
            .call(json!({"action": "invoke", "skill_name": "nonexistent"}))
            .await
            .expect_err("nonexistent");
        assert!(err.to_string().contains("Skill not found"));
    }

    #[tokio::test]
    async fn list_shows_overridden_flag() {
        let mut manager = SkillManager::new();
        // Simulate an override chain: a home skill was overridden by a cwd skill.
        manager.overrides.insert(
            "overridden-skill".into(),
            vec![rara_skills::Skill {
                name: "overridden-skill".into(),
                title: None,
                description: "Old version".into(),
                prompt: "old".into(),
                display_path: "old.md".into(),
                scope: rara_skills::SkillScope::Home,
                disable_model_invocation: false,
            }],
        );
        manager.skills.insert(
            "overridden-skill".into(),
            rara_skills::Skill {
                name: "overridden-skill".into(),
                title: Some("New Version".into()),
                description: "New version".into(),
                prompt: "new".into(),
                display_path: "overridden.md".into(),
                scope: rara_skills::SkillScope::Cwd,
                disable_model_invocation: false,
            },
        );

        let tool = SkillTool {
            skill_manager: Arc::new(manager),
        };

        let result = tool.call(json!({"action": "list"})).await.expect("list");
        let skills = result["skills"].as_array().expect("skills array");
        assert_eq!(skills.len(), 1);

        let skill = &skills[0];
        assert_eq!(skill["name"].as_str(), Some("overridden-skill"));
        assert_eq!(skill["overridden"].as_bool(), Some(true));
        let overridden_by = skill["overridden_by"].as_array().unwrap();
        assert_eq!(overridden_by.len(), 1);
        assert_eq!(overridden_by[0].as_str(), Some("home"));
    }

    #[tokio::test]
    async fn list_returns_active_scopes() {
        let mut manager = SkillManager::new();
        manager.skills.insert(
            "s1".into(),
            rara_skills::Skill {
                name: "s1".into(),
                title: None,
                description: "desc".into(),
                prompt: "body".into(),
                display_path: "s1.md".into(),
                scope: rara_skills::SkillScope::Home,
                disable_model_invocation: false,
            },
        );
        manager.skills.insert(
            "s2".into(),
            rara_skills::Skill {
                name: "s2".into(),
                title: None,
                description: "desc".into(),
                prompt: "body".into(),
                display_path: "s2.md".into(),
                scope: rara_skills::SkillScope::Repo,
                disable_model_invocation: false,
            },
        );

        let tool = SkillTool {
            skill_manager: Arc::new(manager),
        };

        let result = tool.call(json!({"action": "list"})).await.expect("list");
        let scopes = result["scopes"].as_array().expect("scopes array");
        assert_eq!(scopes.len(), 2);
        assert_eq!(scopes[0].as_str(), Some("home"));
        assert_eq!(scopes[1].as_str(), Some("repo"));
    }
}
