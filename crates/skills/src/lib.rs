use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillScope {
    Home,
    Repo,
    Cwd,
    System,
}

#[derive(Debug, Clone, Serialize)]
pub struct Skill {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub prompt: String,
    pub display_path: String,
    pub scope: SkillScope,
    pub disable_model_invocation: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SkillSummary {
    pub name: String,
    pub title: Option<String>,
    pub description: String,
    pub display_path: String,
    pub scope: SkillScope,
    pub disable_model_invocation: bool,
}

pub struct SkillManager {
    pub skills: HashMap<String, Skill>,
    pub overrides: HashMap<String, Vec<Skill>>,
    pub load_warnings: Vec<String>,
}

impl SkillManager {
    pub fn new() -> Self {
        Self {
            skills: HashMap::new(),
            overrides: HashMap::new(),
            load_warnings: Vec::new(),
        }
    }

    pub fn load_all(&mut self) -> Result<()> {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("No home dir"))?;
        let cwd = std::env::current_dir()?;

        let all_dirs = skill_search_dirs(&home, &cwd);
        for dir in &all_dirs {
            if dir.exists() {
                let scope = scope_for_search_dir(dir, &home, &cwd);
                self.load_from_dir(dir, scope)?;
            }
        }
        Ok(())
    }

    pub fn load_from_dir(&mut self, dir: &Path, scope: SkillScope) -> Result<()> {
        let mut skill_files = Vec::new();
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.is_dir() {
                let skill_file = path.join("SKILL.md");
                if skill_file.exists() {
                    skill_files.push(skill_file);
                }
                continue;
            }

            if path.extension().and_then(|s| s.to_str()) == Some("md") {
                skill_files.push(path);
            }
        }

        skill_files
            .sort_by(|left, right| skill_file_sort_key(left).cmp(&skill_file_sort_key(right)));
        for path in skill_files {
            self.load_skill_file(&path, dir, scope)?;
        }
        Ok(())
    }

    fn load_skill_file(&mut self, path: &Path, base_dir: &Path, scope: SkillScope) -> Result<()> {
        let content = fs::read_to_string(path)?;
        let metadata = parse_skill_metadata(content.as_str());
        let name = metadata
            .name
            .clone()
            .unwrap_or_else(|| skill_name_from_path(path));
        let display_path = path
            .strip_prefix(base_dir)
            .unwrap_or(path)
            .display()
            .to_string();

        let new_skill = Skill {
            name: name.clone(),
            title: metadata.title,
            description: metadata.description,
            prompt: content,
            display_path,
            scope,
            disable_model_invocation: metadata.disable_model_invocation,
        };

        if let Some(existing) = self.skills.get(&name) {
            if existing.scope == scope {
                // Same scope: higher-priority file (e.g. SKILL.md) replaces lower-priority file.
                let replaced = self.skills.insert(name.clone(), new_skill).unwrap();
                self.overrides
                    .entry(name.clone())
                    .or_default()
                    .push(replaced);
            } else {
                // Different scope: first scope wins (home > repo > cwd).
                let scope_label = |s: SkillScope| match s {
                    SkillScope::Home => "home",
                    SkillScope::Repo => "repo",
                    SkillScope::Cwd => "cwd",
                    SkillScope::System => "system",
                };
                self.load_warnings.push(format!(
                    "Skill \"{name}\" from {new_scope} ({new_path}) overridden by existing {existing_scope} skill ({existing_path})",
                    name = name,
                    new_scope = scope_label(scope),
                    new_path = path.display(),
                    existing_scope = scope_label(existing.scope),
                    existing_path = existing.display_path,
                ));
                self.overrides
                    .entry(name.clone())
                    .or_default()
                    .push(new_skill);
            }
        } else {
            self.skills.insert(name, new_skill);
        }
        Ok(())
    }

    pub fn get_skill(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn invoke_instructions(&self, name: &str) -> Result<String> {
        self.get_skill(name)
            .map(|skill| skill.prompt.clone())
            .ok_or_else(|| anyhow!("Skill not found: {name}"))
    }

    pub fn list_summaries(&self) -> Vec<SkillSummary> {
        let mut items = self
            .skills
            .values()
            .map(|skill| SkillSummary {
                name: skill.name.clone(),
                title: skill.title.clone(),
                description: skill.description.clone(),
                display_path: skill.display_path.clone(),
                scope: skill.scope,
                disable_model_invocation: skill.disable_model_invocation,
            })
            .collect::<Vec<_>>();
        items.sort_by(|a, b| a.name.cmp(&b.name));
        items
    }

    pub fn list_overrides(&self) -> Vec<SkillSummary> {
        let mut items: Vec<SkillSummary> = self
            .overrides
            .values()
            .flatten()
            .map(|skill| SkillSummary {
                name: skill.name.clone(),
                title: skill.title.clone(),
                description: skill.description.clone(),
                display_path: skill.display_path.clone(),
                scope: skill.scope,
                disable_model_invocation: skill.disable_model_invocation,
            })
            .collect();
        items.sort_by(|a, b| {
            a.name
                .cmp(&b.name)
                .then_with(|| scope_priority(a.scope).cmp(&scope_priority(b.scope)))
        });
        items
    }

    /// Returns the set of scopes that contributed at least one active skill.
    pub fn active_scopes(&self) -> Vec<SkillScope> {
        let mut scopes: Vec<SkillScope> = self
            .skills
            .values()
            .map(|s| s.scope)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        scopes.sort_by_key(|s| scope_priority(*s));
        scopes
    }

    /// Returns whether a skill name has been overridden by a higher-precedence scope.
    pub fn is_overridden(&self, name: &str) -> bool {
        self.overrides.contains_key(name)
    }

    /// Returns the override chain for a skill name (lower-precedence versions that were replaced).
    pub fn override_chain(&self, name: &str) -> Vec<SkillSummary> {
        self.overrides
            .get(name)
            .map(|chain| {
                chain
                    .iter()
                    .map(|skill| SkillSummary {
                        name: skill.name.clone(),
                        title: skill.title.clone(),
                        description: skill.description.clone(),
                        display_path: skill.display_path.clone(),
                        scope: skill.scope,
                        disable_model_invocation: skill.disable_model_invocation,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn scope_for_search_dir(dir: &Path, home: &Path, cwd: &Path) -> SkillScope {
    if dir.starts_with(home) {
        SkillScope::Home
    } else if dir.starts_with(cwd) && dir.ends_with(".rara/skills") {
        SkillScope::Cwd
    } else {
        SkillScope::Repo
    }
}

fn scope_priority(scope: SkillScope) -> u8 {
    match scope {
        SkillScope::Home => 0,
        SkillScope::Repo => 1,
        SkillScope::Cwd => 2,
        SkillScope::System => 3,
    }
}

fn skill_search_dirs(home: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut dirs = vec![
        home.join(".rara/skills"),
        home.join(".agents/skills"),
        home.join(".claude/skills"),
    ];

    for dir in repo_skill_search_dirs(cwd) {
        dirs.push(dir.join(".agents/skills"));
        dirs.push(dir.join(".claude/skills"));
    }

    dirs.push(cwd.join(".rara/skills"));
    dedupe_paths(&mut dirs);
    dirs
}

fn repo_skill_search_dirs(cwd: &Path) -> Vec<PathBuf> {
    let root = find_project_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let mut dirs = Vec::new();
    let mut current = Some(cwd);
    while let Some(dir) = current {
        dirs.push(dir.to_path_buf());
        if dir == root {
            break;
        }
        current = dir.parent();
    }
    dirs.reverse();
    dirs
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut current = Some(start);
    while let Some(dir) = current {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
        current = dir.parent();
    }
    None
}

fn dedupe_paths(paths: &mut Vec<PathBuf>) {
    let mut seen = HashSet::<PathBuf>::new();
    paths.retain(|path| seen.insert(path.clone()));
}

fn skill_name_from_path(path: &Path) -> String {
    if path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md") {
        path.parent()
            .and_then(|value| value.file_name())
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        path.file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("unknown")
            .to_string()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedSkillMetadata {
    name: Option<String>,
    title: Option<String>,
    description: String,
    disable_model_invocation: bool,
}

fn parse_skill_metadata(content: &str) -> ParsedSkillMetadata {
    let (frontmatter, markdown) = split_frontmatter(content);
    let name = frontmatter.and_then(|frontmatter| frontmatter_value(frontmatter, "name"));
    let title = frontmatter
        .and_then(|frontmatter| {
            frontmatter_value(frontmatter, "title")
                .or_else(|| frontmatter_value(frontmatter, "display_name"))
        })
        .or_else(|| first_markdown_heading(markdown));
    let description = frontmatter
        .and_then(|frontmatter| frontmatter_value(frontmatter, "description"))
        .or_else(|| title.clone())
        .or_else(|| first_non_empty_markdown_line(markdown))
        .unwrap_or_else(|| "No description".to_string());
    let disable_model_invocation = frontmatter
        .and_then(|frontmatter| frontmatter_value(frontmatter, "disable_model_invocation"))
        .map(|v| v == "true")
        .unwrap_or(false);

    ParsedSkillMetadata {
        name,
        title,
        description,
        disable_model_invocation,
    }
}

fn split_frontmatter(content: &str) -> (Option<&str>, &str) {
    let mut lines = content.split_inclusive('\n');
    let Some(first_line) = lines.next() else {
        return (None, content);
    };

    if frontmatter_delimiter(first_line) != "---" {
        return (None, content);
    }

    let frontmatter_start = first_line.len();
    let mut cursor = frontmatter_start;

    for line in lines {
        let line_start = cursor;
        cursor += line.len();

        if frontmatter_delimiter(line) == "---" {
            let frontmatter = content[frontmatter_start..line_start].trim_end_matches(['\r', '\n']);
            return (Some(frontmatter), &content[cursor..]);
        }
    }

    (None, content)
}

fn frontmatter_delimiter(line: &str) -> &str {
    line.trim_end_matches('\n').trim_end_matches('\r')
}

fn frontmatter_value(frontmatter: &str, key: &str) -> Option<String> {
    frontmatter.lines().find_map(|line| {
        let trimmed = line.trim();
        let (candidate_key, value) = trimmed.split_once(':')?;
        (candidate_key.trim() == key).then(|| clean_frontmatter_value(value))?
    })
}

fn clean_frontmatter_value(value: &str) -> Option<String> {
    let value = value.trim().trim_matches('"').trim_matches('\'').trim();
    (!value.is_empty()).then(|| value.to_string())
}

fn first_markdown_heading(markdown: &str) -> Option<String> {
    markdown.lines().find_map(|line| {
        let trimmed = line.trim();
        let hashes = trimmed.chars().take_while(|ch| *ch == '#').count();
        if hashes == 0 || hashes > 6 {
            return None;
        }
        let title = trimmed[hashes..].trim();
        (!title.is_empty()).then(|| truncate_metadata_value(title))
    })
}

fn first_non_empty_markdown_line(markdown: &str) -> Option<String> {
    markdown
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .map(|line| {
            let hashes = line.chars().take_while(|ch| *ch == '#').count();
            if hashes > 0 && hashes <= 6 {
                truncate_metadata_value(line[hashes..].trim())
            } else {
                truncate_metadata_value(line)
            }
        })
}

fn truncate_metadata_value(value: &str) -> String {
    const MAX_LEN: usize = 100;
    if value.chars().count() <= MAX_LEN {
        return value.to_string();
    }

    let mut truncated = value.chars().take(MAX_LEN - 3).collect::<String>();
    truncated.push_str("...");
    truncated
}

fn skill_file_sort_key(path: &Path) -> (String, u8, &Path) {
    let name = skill_name_from_path(path);
    let priority = if path.file_name().and_then(|value| value.to_str()) == Some("SKILL.md") {
        1
    } else {
        0
    };
    (name, priority, path)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{SkillManager, SkillScope, repo_skill_search_dirs, skill_search_dirs};

    #[test]
    fn loads_legacy_markdown_skills() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("legacy.md"), "# Legacy Skill\nbody").expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("legacy").expect("legacy skill");
        assert_eq!(skill.title.as_deref(), Some("Legacy Skill"));
        assert_eq!(skill.description, "Legacy Skill");
        assert_eq!(skill.display_path, "legacy.md");
        assert_eq!(skill.scope, SkillScope::Cwd);
    }

    #[test]
    fn loads_directory_skills_from_skill_md() {
        let dir = tempdir().expect("tempdir");
        let skill_dir = dir.path().join("reviewer");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(skill_dir.join("SKILL.md"), "# Reviewer\nworkflow").expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("reviewer").expect("reviewer skill");
        assert_eq!(skill.title.as_deref(), Some("Reviewer"));
        assert_eq!(skill.description, "Reviewer");
        assert_eq!(skill.display_path, "reviewer/SKILL.md");
        assert_eq!(skill.scope, SkillScope::Cwd);
    }

    #[test]
    fn loads_codex_style_frontmatter_metadata() {
        let dir = tempdir().expect("tempdir");
        let skill_dir = dir.path().join("reviewer");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: code-review\ntitle: Code Review\ndescription: Review local code changes.\n---\n\n# Ignored Heading\nworkflow",
        )
        .expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("code-review").expect("frontmatter skill");
        assert_eq!(skill.title.as_deref(), Some("Code Review"));
        assert_eq!(skill.description, "Review local code changes.");
        assert_eq!(skill.display_path, "reviewer/SKILL.md");
        assert_eq!(skill.scope, SkillScope::Cwd);
    }

    #[test]
    fn loads_frontmatter_metadata_with_crlf_line_endings() {
        let dir = tempdir().expect("tempdir");
        let skill_dir = dir.path().join("reviewer");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\r\nname: windows-review\r\ntitle: Windows Review\r\ndescription: Review CRLF metadata.\r\n---\r\n\r\n# Ignored Heading\r\nworkflow",
        )
        .expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager
            .get_skill("windows-review")
            .expect("frontmatter skill");
        assert_eq!(skill.title.as_deref(), Some("Windows Review"));
        assert_eq!(skill.description, "Review CRLF metadata.");
        assert_eq!(skill.display_path, "reviewer/SKILL.md");
        assert_eq!(skill.scope, SkillScope::Cwd);
    }

    #[test]
    fn load_from_dir_prefers_directory_skill_over_legacy_markdown_with_same_name() {
        let dir = tempdir().expect("tempdir");
        let skill_dir = dir.path().join("reviewer");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(dir.path().join("reviewer.md"), "# Legacy Reviewer\nlegacy").expect("write");
        fs::write(skill_dir.join("SKILL.md"), "# Directory Reviewer\nworkflow").expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("reviewer").expect("reviewer skill");
        assert_eq!(skill.description, "Directory Reviewer");
        assert_eq!(skill.display_path, "reviewer/SKILL.md");
        assert_eq!(skill.scope, SkillScope::Cwd);
    }

    #[test]
    fn overrides_track_duplicate_skill_across_scopes() {
        let home_dir = tempdir().expect("tempdir");
        let cwd_dir = tempdir().expect("tempdir");
        let home_skills = home_dir.path().join(".rara/skills");
        let cwd_skills = cwd_dir.path().join(".rara/skills");
        fs::create_dir_all(&home_skills).expect("mkdir home");
        fs::create_dir_all(&cwd_skills).expect("mkdir cwd");

        fs::write(
            home_skills.join("reviewer.md"),
            "# Home Reviewer\nhome workflow",
        )
        .expect("write home");
        fs::write(
            cwd_skills.join("reviewer.md"),
            "# Cwd Reviewer\ncwd workflow",
        )
        .expect("write cwd");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(home_skills.as_path(), SkillScope::Home)
            .expect("load home");
        manager
            .load_from_dir(cwd_skills.as_path(), SkillScope::Cwd)
            .expect("load cwd");

        let skill = manager.get_skill("reviewer").expect("reviewer skill");
        assert_eq!(skill.description, "Home Reviewer");
        assert_eq!(skill.scope, SkillScope::Home);

        let overrides = manager.overrides.get("reviewer").expect("override entry");
        assert_eq!(overrides.len(), 1);
        assert_eq!(overrides[0].description, "Cwd Reviewer");
        assert_eq!(overrides[0].scope, SkillScope::Cwd);

        let warnings: Vec<&str> = manager
            .load_warnings
            .iter()
            .filter(|w| w.contains("reviewer"))
            .map(|s| s.as_str())
            .collect();
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("overridden by existing home")),
            "expected override warning, got: {:?}",
            warnings
        );
    }

    #[test]
    fn scope_priority_home_wins_over_repo_and_cwd() {
        let dir = tempdir().expect("tempdir");
        fs::write(dir.path().join("shared.md"), "# First\nfirst body").expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Home)
            .expect("first load");
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("second load");

        let skill = manager.get_skill("shared").expect("shared skill");
        assert_eq!(skill.scope, SkillScope::Home);
        assert_eq!(skill.description, "First");
    }

    #[test]
    fn search_dirs_include_claude_skill_roots() {
        let temp = tempdir().expect("tempdir");
        let home = temp.path().join("home");
        let repo = temp.path().join("repo");
        let nested = repo.join("crates/skills");
        fs::create_dir_all(&home).expect("mkdir home");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::create_dir_all(repo.join(".git")).expect("mkdir git");

        let dirs = skill_search_dirs(&home, &nested);
        assert_eq!(
            dirs,
            vec![
                home.join(".rara/skills"),
                home.join(".agents/skills"),
                home.join(".claude/skills"),
                repo.join(".agents/skills"),
                repo.join(".claude/skills"),
                repo.join("crates/.agents/skills"),
                repo.join("crates/.claude/skills"),
                nested.join(".agents/skills"),
                nested.join(".claude/skills"),
                nested.join(".rara/skills"),
            ]
        );
    }

    #[test]
    fn repo_skill_search_dirs_walks_from_project_root_to_cwd() {
        let temp = tempdir().expect("tempdir");
        let repo = temp.path().join("repo");
        let nested = repo.join("a/b");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::create_dir_all(repo.join(".git")).expect("mkdir git");

        let dirs = repo_skill_search_dirs(&nested);
        assert_eq!(dirs, vec![repo.clone(), repo.join("a"), nested]);
    }

    #[test]
    fn loads_frontmatter_disable_model_invocation() {
        let dir = tempdir().expect("tempdir");
        let skill_dir = dir.path().join("restricted");
        fs::create_dir_all(&skill_dir).expect("mkdir");
        fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: restricted\ndisable_model_invocation: true\n---\n\n# Restricted\nbody",
        )
        .expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("restricted").expect("restricted skill");
        assert!(skill.disable_model_invocation);
    }

    #[test]
    fn loads_frontmatter_disable_model_invocation_false_by_default() {
        let dir = tempdir().expect("tempdir");
        fs::write(
            dir.path().join("open.md"),
            "---\nname: open\ntitle: Open\n---\n\nbody",
        )
        .expect("write");

        let mut manager = SkillManager::new();
        manager
            .load_from_dir(dir.path(), SkillScope::Cwd)
            .expect("load");

        let skill = manager.get_skill("open").expect("open skill");
        assert!(!skill.disable_model_invocation);
    }
}
