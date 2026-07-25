use std::path::PathBuf;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use rara_tools::tool::{Tool, ToolError};
use serde_json::{Value, json};

use crate::skill::SkillManager;

#[cfg(test)]
fn shared_skill_manager(manager: SkillManager) -> Arc<RwLock<SkillManager>> {
    Arc::new(RwLock::new(manager))
}

pub struct SkillTool {
    pub skill_manager: Arc<RwLock<SkillManager>>,
    pub plugin_roots: Vec<(String, PathBuf)>,
}
#[tool_spec(
    name = "skill",
    description = "Manage and invoke reusable skills. Skills are stored in SKILL.md files across home, repo, and cwd scopes. Higher-precedence scopes override lower-precedence skills with the same name. Use list to discover available skills, invoke to load instructions, reload to re-scan after file changes. If a user names a skill or uses slash-command shorthand like /review, or if the request clearly matches an available skill, invoke that exact listed skill before doing task-specific work. Never invent skill names from memory or training data, and never mention that a skill applies unless you actually invoke it or it has already been injected in the current turn.",
    input_schema = {
        "type": "object",
        "properties": {
            "action": {
                "type": "string",
                "enum": ["list", "invoke", "reload"],
                "description": "list: discover available skills with their scopes, override status, and metadata. invoke: load a specific skill's full instructions by name. reload: re-scan skill directories after file changes."
            },
            "skill_name": {
                "type": "string",
                "description": "Exact name of the available skill to invoke, without a leading slash. Required when action is invoke. Do not guess names that were not returned by list or explicitly typed by the user."
            },
            "args": {
                "type": "string",
                "description": "Optional arguments to pass to the invoked skill (for slash-command-style skills)."
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
                let skill_manager = self.skill_manager.read().map_err(|err| {
                    ToolError::ExecutionFailed(format!("skill lock failed: {err}"))
                })?;
                let scopes = skill_manager.active_scopes();
                let skills: Vec<Value> = skill_manager
                    .list_summaries()
                    .iter()
                    .map(|s| {
                        let shadows = skill_manager.shadows_others(&s.name);
                        json!({
                            "name": s.name,
                            "title": s.title,
                            "description": s.description,
                            "scope": s.scope.as_str(),
                            "disable_model_invocation": s.disable_model_invocation,
                            "overrides_others": shadows,
                            "shadowed_scopes": if shadows {
                                skill_manager.override_chain(&s.name)
                                    .iter()
                                    .map(|o| o.scope.as_str())
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
                    "overrides": skill_manager.list_overrides(),
                    "load_warnings": &skill_manager.load_warnings,
                }))
            }
            "invoke" => {
                let name = i["skill_name"]
                    .as_str()
                    .ok_or(ToolError::InvalidInput("skill_name".into()))?;
                let args = i.get("args").and_then(|v| v.as_str()).map(String::from);
                let skill_manager = self.skill_manager.read().map_err(|err| {
                    ToolError::ExecutionFailed(format!("skill lock failed: {err}"))
                })?;
                let skill = skill_manager
                    .get_skill(name)
                    .ok_or(ToolError::ExecutionFailed(format!(
                        "Skill not found: {name}"
                    )))?;
                let shadowed_scopes: Vec<String> = skill_manager
                    .override_chain(name)
                    .iter()
                    .map(|o| o.scope.as_str().to_string())
                    .collect();
                Ok(json!({
                    "name": skill.name,
                    "title": skill.title,
                    "scope": skill.scope.as_str(),
                    "instructions": skill.instructions(),
                    "args": args,
                    "disable_model_invocation": skill.disable_model_invocation,
                    "overrides_others": !shadowed_scopes.is_empty(),
                    "shadowed_scopes": shadowed_scopes,
                }))
            }
            "reload" => {
                let mut verify = SkillManager::new();
                if let Err(err) = verify.load_all() {
                    return Err(ToolError::ExecutionFailed(err.to_string()));
                }
                for (plugin_name, plugin_root) in &self.plugin_roots {
                    if let Err(err) = verify.load_plugin_skills_from_root(plugin_name, plugin_root)
                    {
                        verify.load_warnings.push(format!(
                            "Failed to load plugin skills from {}: {}",
                            plugin_root.display(),
                            err
                        ));
                    }
                }
                let skill_count = verify.list_summaries().len();
                let warnings = verify.load_warnings.clone();
                let mut skill_manager = self.skill_manager.write().map_err(|err| {
                    ToolError::ExecutionFailed(format!("skill lock failed: {err}"))
                })?;
                *skill_manager = verify;
                Ok(json!({
                    "reloaded": true,
                    "skill_count": skill_count,
                    "warnings": warnings,
                }))
            }
            _ => Err(ToolError::InvalidInput("Invalid action".into())),
        }
    }
}

#[tokio::test]
async fn list_returns_scopes_and_skills() {
    let mut manager = SkillManager::new();
    manager.load_warnings = vec!["test warning".into()];

    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
    };

    let result = tool.call(json!({"action": "list"})).await.expect("list");
    let skills = result["skills"].as_array().expect("skills array");
    let scopes = result["scopes"].as_array().expect("scopes array");
    assert!(skills.is_empty());
    assert!(scopes.is_empty());
    assert_eq!(result["load_warnings"][0].as_str(), Some("test warning"));
}

#[test]
fn skill_tool_description_requires_exact_pre_task_invocation() {
    let manager = SkillManager::new();
    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
    };
    let description = tool.description();

    assert!(description.contains("slash-command shorthand"));
    assert!(description.contains("invoke that exact listed skill before doing task-specific work"));
    assert!(description.contains("Never invent skill names"));
    assert!(
        description.contains("never mention that a skill applies unless you actually invoke it")
    );

    let schema = tool.input_schema().to_string();
    assert!(schema.contains("without a leading slash"));
    assert!(schema.contains("Do not guess names"));
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
            path: std::path::PathBuf::from("test-skill/SKILL.md"),
            scope: rara_skills::SkillScope::Cwd,
            content: "# Test\nbody".into(),
            disable_model_invocation: false,
        },
    );

    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
    };

    let result = tool
        .call(json!({"action": "invoke", "skill_name": "test-skill"}))
        .await
        .expect("invoke");

    assert_eq!(result["name"].as_str(), Some("test-skill"));
    assert_eq!(result["title"].as_str(), Some("Test Skill"));
    assert_eq!(result["scope"].as_str(), Some("cwd"));
    assert_eq!(result["instructions"].as_str(), Some("# Test\nbody"));
    assert_eq!(result["overrides_others"].as_bool(), Some(false));
    assert!(result["shadowed_scopes"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn invoke_missing_skill_returns_error() {
    let manager = SkillManager::new();
    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
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
            path: std::path::PathBuf::from("old.md"),
            scope: rara_skills::SkillScope::Home,
            content: "old".into(),
            disable_model_invocation: false,
        }],
    );
    manager.skills.insert(
        "overridden-skill".into(),
        rara_skills::Skill {
            name: "overridden-skill".into(),
            title: Some("New Version".into()),
            description: "New version".into(),
            path: std::path::PathBuf::from("overridden.md"),
            scope: rara_skills::SkillScope::Cwd,
            content: "new".into(),
            disable_model_invocation: false,
        },
    );

    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
    };

    let result = tool.call(json!({"action": "list"})).await.expect("list");
    let skills = result["skills"].as_array().expect("skills array");
    assert_eq!(skills.len(), 1);

    let skill = &skills[0];
    assert_eq!(skill["name"].as_str(), Some("overridden-skill"));
    assert_eq!(skill["overrides_others"].as_bool(), Some(true));
    let shadowed = skill["shadowed_scopes"].as_array().unwrap();
    assert_eq!(shadowed.len(), 1);
    assert_eq!(shadowed[0].as_str(), Some("home"));
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
            path: std::path::PathBuf::from("s1.md"),
            scope: rara_skills::SkillScope::Home,
            content: "body".into(),
            disable_model_invocation: false,
        },
    );
    manager.skills.insert(
        "s2".into(),
        rara_skills::Skill {
            name: "s2".into(),
            title: None,
            description: "desc".into(),
            path: std::path::PathBuf::from("s2.md"),
            scope: rara_skills::SkillScope::Repo,
            content: "body".into(),
            disable_model_invocation: false,
        },
    );

    let tool = SkillTool {
        skill_manager: shared_skill_manager(manager),
        plugin_roots: Vec::new(),
    };

    let result = tool.call(json!({"action": "list"})).await.expect("list");
    let scopes = result["scopes"].as_array().expect("scopes array");
    assert_eq!(scopes.len(), 2);
    assert_eq!(scopes[0].as_str(), Some("home"));
    assert_eq!(scopes[1].as_str(), Some("repo"));
}

#[tokio::test]
async fn reload_updates_running_manager_with_plugin_skills() {
    let temp = tempfile::tempdir().expect("tempdir");
    let plugin_root = temp.path().join("plugin");
    let skill_dir = plugin_root.join("skills").join("reviewer");
    std::fs::create_dir_all(&skill_dir).expect("skill dir");
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# Reviewer\nInspect plugin-provided behavior.",
    )
    .expect("skill");

    let manager = shared_skill_manager(SkillManager::new());
    let tool = SkillTool {
        skill_manager: manager.clone(),
        plugin_roots: vec![("quality".to_string(), plugin_root)],
    };

    let result = tool
        .call(json!({"action": "reload"}))
        .await
        .expect("reload");
    assert_eq!(result["reloaded"].as_bool(), Some(true));

    let invoked = tool
        .call(json!({"action": "invoke", "skill_name": "quality:reviewer"}))
        .await
        .expect("invoke plugin skill");
    assert_eq!(invoked["scope"].as_str(), Some("plugin"));
    assert_eq!(
        invoked["instructions"].as_str(),
        Some("# Reviewer\nInspect plugin-provided behavior.")
    );

    let guard = manager.read().expect("manager");
    assert!(guard.get_skill("quality:reviewer").is_some());
}
