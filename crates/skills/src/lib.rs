use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Workspace,
    Global,
    // Legacy variants for compatibility
    Home,
    Repo,
    Cwd,
    System,
}

impl SkillScope {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Global => "global",
            Self::Home => "home",
            Self::Repo => "repo",
            Self::Cwd => "cwd",
            Self::System => "system",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub path: PathBuf,
    pub display_path: String,
    pub scope: SkillScope,
    pub content: String,
    pub prompt: String,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub path: PathBuf,
    pub display_path: String,
    pub scope: SkillScope,
    pub disable_model_invocation: bool,
}

pub struct SkillManager {
    pub skills: HashMap<String, Skill>,
    pub overrides: HashMap<String, Vec<Skill>>,
    pub load_warnings: Vec<String>,
}

impl Default for SkillManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SkillManager {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            overrides: HashMap::new(),
            load_warnings: Vec::new(),
        }
    }

    pub fn discover_and_load(&mut self) -> Result<()> {
        let workspace_root = std::env::current_dir()?;
        self.discover_workspace_skills(&workspace_root)?;
        self.discover_global_skills()?;
        self.load_bundled_skills()?;
        Ok(())
    }

    pub fn load_all(&mut self) -> Result<()> {
        self.discover_and_load()
    }

    pub fn list_summaries(&self) -> Vec<SkillSummary> {
        self.skills
            .values()
            .map(|s| SkillSummary {
                name: s.name.clone(),
                title: s.title.clone(),
                description: s.description.clone(),
                path: s.path.clone(),
                display_path: s.display_path.clone(),
                scope: s.scope,
                disable_model_invocation: s.disable_model_invocation,
            })
            .collect()
    }

    pub fn get_skill(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn active_scopes(&self) -> Vec<String> {
        let mut scopes = HashSet::new();
        for skill in self.skills.values() {
            scopes.insert(skill.scope.as_str().to_string()); 
        }
        let mut result: Vec<String> = scopes.into_iter().collect();
        result.sort();
        result
    }

    pub fn shadows_others(&self, name: &str) -> bool {
        self.overrides.contains_key(name)
    }

    pub fn override_chain(&self, name: &str) -> Vec<Skill> {
        self.overrides.get(name).cloned().unwrap_or_default()
    }

    pub fn list_overrides(&self) -> HashMap<String, Vec<String>> {
        let mut result = HashMap::new();
        for (name, chain) in &self.overrides {
            result.insert(
                name.clone(),
                chain
                    .iter()
                    .map(|s| format!("{} ({})", s.name, s.scope.as_str()))
                    .collect(),
            );
        }
        result
    }

    fn discover_workspace_skills(&mut self, root: &Path) -> Result<()> {
        let agents_dir = root.join(".agents").join("skills");
        if agents_dir.is_dir() {
            self.load_skills_from_dir(&agents_dir, SkillScope::Workspace)?;
        }
        Ok(())
    }

    fn discover_global_skills(&mut self) -> Result<()> {
        if let Ok(home) = std::env::var("HOME") {
            let global_dir = Path::new(&home).join(".agents").join("skills");
            if global_dir.is_dir() {
                self.load_skills_from_dir(&global_dir, SkillScope::Global)?;
            }
        }
        Ok(())
    }

    fn load_skills_from_dir(&mut self, dir: &Path, scope: SkillScope) -> Result<()> {
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let skill_md = path.join("SKILL.md");
                if skill_md.is_file() {
                    match self.load_skill(&skill_md, scope) {
                        Ok(skill) => {
                            let name = skill.name.clone();
                            if let Some(existing) = self.skills.get(&name) {
                                let mut chain = self.overrides.remove(&name).unwrap_or_default();
                                if scope == SkillScope::Workspace && existing.scope == SkillScope::Global {
                                    chain.push(existing.clone());
                                    self.skills.insert(name.clone(), skill);
                                } else {
                                    chain.push(skill);
                                }
                                self.overrides.insert(name, chain);
                            } else {
                                self.skills.insert(name, skill);
                            }
                        }
                        Err(err) => {
                            self.load_warnings.push(format!(
                                "Failed to load skill from {}: {}",
                                skill_md.display(),
                                err
                            ));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    fn load_skill(&self, path: &Path, scope: SkillScope) -> Result<Skill> {
        let content = fs::read_to_string(path)?;
        let name = path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            .ok_or_else(|| anyhow!("invalid skill path"))?
            .to_string();

        let description = extract_description(&content).unwrap_or_else(|| "No description provided.".to_string());

        Ok(Skill {
            name: name.clone(),
            title: Some(name),
            description,
            path: path.to_path_buf(),
            display_path: path.display().to_string(),
            scope,
            content: content.clone(),
            prompt: content,
            disable_model_invocation: false,
        })
    }

    fn load_bundled_skills(&mut self) -> Result<()> {
        // Placeholder for bundled system skills (e.g. verify)
        Ok(())
    }
}

fn extract_description(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if !trimmed.is_empty() && !trimmed.starts_with('#') {
            return Some(trimmed.to_string());
        }
    }
    None
}

// -- Bundled System Skills ----------------------------------------

const SYSTEM_SKILL_VERIFY: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/skills/verify/SKILL.md"
));
const SYSTEM_SKILL_VERIFIER_GENERIC: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../assets/skills/verifier-generic/SKILL.md"
));
