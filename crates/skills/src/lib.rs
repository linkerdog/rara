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
    pub scope: SkillScope,
    pub content: String,
    pub disable_model_invocation: bool,
}

impl Skill {
    pub fn instructions(&self) -> String {
        strip_frontmatter(&self.content)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub path: PathBuf,
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
                                if scope == SkillScope::Workspace
                                    && existing.scope == SkillScope::Global
                                {
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

        let description =
            extract_description(&content).unwrap_or_else(|| "No description provided.".to_string());

        Ok(Skill {
            name: name.clone(),
            title: Some(name),
            description,
            path: path.to_path_buf(),
            scope,
            content,
            disable_model_invocation: false,
        })
    }

    fn load_bundled_skills(&mut self) -> Result<()> {
        // Placeholder for bundled system skills (e.g. verify)
        Ok(())
    }
}

pub fn strip_frontmatter(content: &str) -> String {
    let mut in_frontmatter = false;
    let mut frontmatter_count = 0;
    let mut result_lines = Vec::new();

    for line in content.lines() {
        if line.trim() == "---" {
            frontmatter_count += 1;
            if frontmatter_count == 1 {
                in_frontmatter = true;
                continue;
            } else if frontmatter_count == 2 {
                in_frontmatter = false;
                continue;
            }
        }
        if !in_frontmatter {
            result_lines.push(line);
        }
    }
    result_lines.join("\n").trim().to_string()
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

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn test_strip_frontmatter() {
        let content = "---\ntitle: Test\n---\n# Body\nHello";
        assert_eq!(strip_frontmatter(content), "# Body\nHello");

        let no_frontmatter = "# Body\nHello";
        assert_eq!(strip_frontmatter(no_frontmatter), "# Body\nHello");
    }

    #[test]
    fn test_skill_discovery() -> Result<()> {
        let temp = tempdir()?;
        let skills_dir = temp.path().join(".agents").join("skills");
        let skill_path = skills_dir.join("test-skill");
        fs::create_dir_all(&skill_path)?;
        fs::write(
            skill_path.join("SKILL.md"),
            "# Test Skill\nThis is a description.",
        )?;

        let mut manager = SkillManager::new();
        manager.discover_workspace_skills(temp.path())?;

        assert_eq!(manager.skills.len(), 1);
        let skill = manager.get_skill("test-skill").unwrap();
        assert_eq!(skill.name, "test-skill");
        assert_eq!(skill.description, "This is a description.");
        Ok(())
    }

    #[test]
    fn test_skill_override() -> Result<()> {
        let mut manager = SkillManager::new();

        let s1 = Skill {
            name: "s".into(),
            title: None,
            description: "d1".into(),
            path: PathBuf::from("p1"),
            scope: SkillScope::Global,
            content: "c1".into(),
            disable_model_invocation: false,
        };
        manager.skills.insert(s1.name.clone(), s1);

        let s2 = Skill {
            name: "s".into(),
            title: None,
            description: "d2".into(),
            path: PathBuf::from("p2"),
            scope: SkillScope::Workspace,
            content: "c2".into(),
            disable_model_invocation: false,
        };

        // Manual insertion logic check
        let name = s2.name.clone();
        if let Some(existing) = manager.skills.get(&name) {
            let mut chain = manager.overrides.remove(&name).unwrap_or_default();
            chain.push(existing.clone());
            manager.skills.insert(name.clone(), s2);
            manager.overrides.insert(name, chain);
        }

        assert_eq!(manager.skills.get("s").unwrap().content, "c2");
        assert_eq!(manager.overrides.get("s").unwrap().len(), 1);
        assert_eq!(manager.overrides.get("s").unwrap()[0].content, "c1");
        Ok(())
    }
}
