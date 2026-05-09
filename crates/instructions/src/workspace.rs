use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::SystemTime;

use anyhow::Result;
use rara_config::workspace_data_dir_for;

use crate::prompt::{PromptSource, PromptSourceKind};

const PROJECT_INSTRUCTION_FILES: [&str; 3] = ["CLAUDE.md", "GEMINI.md", "AGENTS.md"];
const USER_INSTRUCTION_FILE: &str = "AGENTS.md";

pub struct WorkspaceMemory {
    pub root: PathBuf,
    pub rara_dir: PathBuf,
    cache: Mutex<WorkspaceCache>,
}

#[derive(Default)]
struct WorkspaceCache {
    prompt_files: HashMap<PathBuf, CachedTextFile>,
    env_info: Option<CachedEnvInfo>,
    memory_file_available: Option<bool>,
}

#[derive(Clone)]
struct CachedTextFile {
    modified: Option<SystemTime>,
    content: String,
}

#[derive(Clone)]
struct CachedEnvInfo {
    git_head_marker: Option<SystemTime>,
    cwd: String,
    branch: String,
}

impl WorkspaceMemory {
    pub fn new() -> Result<Self> {
        let root = std::env::current_dir()?;
        let rara_dir = workspace_data_dir_for(&root)?;
        Ok(Self::from_paths(root, rara_dir))
    }

    pub fn from_paths(root: PathBuf, rara_dir: PathBuf) -> Self {
        Self {
            root,
            rara_dir,
            cache: Mutex::new(WorkspaceCache::default()),
        }
    }

    pub fn read_memory_file(&self) -> Option<String> {
        let path = self.rara_dir.join("memory.md");
        let content = fs::read_to_string(path).ok();
        if let Ok(mut cache) = self.cache.lock() {
            cache.memory_file_available = Some(content.is_some());
        }
        content
    }

    pub fn write_memory_file(&self, content: &str) -> Result<()> {
        let path = self.rara_dir.join("memory.md");
        fs::write(path, content)?;
        if let Ok(mut cache) = self.cache.lock() {
            cache.memory_file_available = Some(true);
        }
        Ok(())
    }

    pub fn has_memory_file_cached(&self) -> bool {
        let mut cache = self.cache.lock().expect("workspace cache poisoned");
        if let Some(available) = cache.memory_file_available {
            return available;
        }

        let available = self.rara_dir.join("memory.md").exists();
        cache.memory_file_available = Some(available);
        available
    }

    pub fn discover_instructions(&self) -> Vec<String> {
        self.discover_prompt_sources()
            .into_iter()
            .filter(|source| {
                matches!(
                    source.kind,
                    PromptSourceKind::UserInstruction | PromptSourceKind::ProjectInstruction
                )
            })
            .map(|source| format!("### {}:\n{}", source.label, source.content))
            .collect()
    }

    pub fn discover_prompt_sources(&self) -> Vec<PromptSource> {
        let focus = self.focus_dir();
        self.discover_prompt_sources_from_dir(&focus)
    }

    fn discover_prompt_sources_from_dir(&self, focus: &Path) -> Vec<PromptSource> {
        let mut sources = Vec::new();
        sources.extend(self.user_instruction_sources());
        for dir in self.instruction_search_dirs(focus) {
            for file in PROJECT_INSTRUCTION_FILES {
                let path = dir.join(file);
                if let Some(content) = self.cached_file_content(&path) {
                    let display_path = path
                        .strip_prefix(&self.root)
                        .unwrap_or(&path)
                        .display()
                        .to_string();
                    sources.push(PromptSource {
                        kind: PromptSourceKind::ProjectInstruction,
                        label: format!("Project Instruction ({file})"),
                        display_path,
                        content,
                    });
                }
            }
        }
        let memory = self.rara_dir.join("memory.md");
        if let Some(content) = self.cached_file_content(&memory) {
            sources.push(PromptSource {
                kind: PromptSourceKind::LocalMemory,
                label: "Local Project Memory".to_string(),
                display_path: memory.display().to_string(),
                content,
            });
        }
        sources
    }

    fn user_instruction_sources(&self) -> Vec<PromptSource> {
        let Some(rara_home) = self.rara_home_dir() else {
            return Vec::new();
        };
        let path = rara_home.join(USER_INSTRUCTION_FILE);
        let Some(content) = self.cached_file_content(&path) else {
            return Vec::new();
        };

        vec![PromptSource {
            kind: PromptSourceKind::UserInstruction,
            label: format!("User Instruction ({USER_INSTRUCTION_FILE})"),
            display_path: path.display().to_string(),
            content,
        }]
    }

    pub fn get_env_info(&self) -> (String, String) {
        let cwd = self.root.display().to_string();
        let marker = self.git_head_marker();
        let mut cache = self.cache.lock().expect("workspace cache poisoned");
        if let Some(cached) = cache.env_info.as_ref() {
            if cached.cwd == cwd && cached.git_head_marker == marker {
                return (cached.cwd.clone(), cached.branch.clone());
            }
        }

        let branch = self.read_git_branch();
        cache.env_info = Some(CachedEnvInfo {
            git_head_marker: marker,
            cwd: cwd.clone(),
            branch: branch.clone(),
        });
        (cwd, branch)
    }

    fn cached_file_content(&self, path: &Path) -> Option<String> {
        let modified = fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok());
        let mut cache = self.cache.lock().expect("workspace cache poisoned");
        if let Some(cached) = cache.prompt_files.get(path) {
            if cached.modified == modified {
                return Some(cached.content.clone());
            }
        }

        let content = fs::read_to_string(path).ok()?;
        cache.prompt_files.insert(
            path.to_path_buf(),
            CachedTextFile {
                modified,
                content: content.clone(),
            },
        );
        Some(content)
    }

    fn read_git_branch(&self) -> String {
        let Some(git_dir) = self.resolve_git_dir() else {
            return "no-git".to_string();
        };
        let head = git_dir.join("HEAD");
        let Ok(head_text) = fs::read_to_string(head) else {
            return "no-git".to_string();
        };
        let head_text = head_text.trim();
        if let Some(reference) = head_text.strip_prefix("ref: ") {
            return reference
                .strip_prefix("refs/heads/")
                .unwrap_or(reference)
                .to_string();
        }
        if head_text.is_empty() {
            "no-git".to_string()
        } else {
            format!("detached@{}", &head_text[..head_text.len().min(12)])
        }
    }

    fn git_head_marker(&self) -> Option<SystemTime> {
        let git_dir = self.resolve_git_dir()?;
        fs::metadata(git_dir.join("HEAD"))
            .ok()
            .and_then(|meta| meta.modified().ok())
    }

    fn resolve_git_dir(&self) -> Option<PathBuf> {
        let dot_git = self.root.join(".git");
        let metadata = fs::metadata(&dot_git).ok()?;
        if metadata.is_dir() {
            return Some(dot_git);
        }
        let raw = fs::read_to_string(&dot_git).ok()?;
        let value = raw.strip_prefix("gitdir:")?.trim();
        let git_dir = Path::new(value);
        if git_dir.is_absolute() {
            Some(git_dir.to_path_buf())
        } else {
            Some(self.root.join(git_dir))
        }
    }

    fn instruction_search_dirs(&self, focus: &Path) -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        let mut current = Some(focus);
        while let Some(dir) = current {
            if !dir.starts_with(&self.root) {
                break;
            }
            dirs.push(dir.to_path_buf());
            if dir == self.root {
                break;
            }
            current = dir.parent();
        }
        dirs.reverse();
        dirs
    }

    fn rara_home_dir(&self) -> Option<PathBuf> {
        let workspaces_dir = self.rara_dir.parent()?;
        if workspaces_dir.file_name()? != "workspaces" {
            return None;
        }
        workspaces_dir.parent().map(Path::to_path_buf)
    }

    fn focus_dir(&self) -> PathBuf {
        let Ok(cwd) = std::env::current_dir() else {
            return self.root.clone();
        };
        if cwd.starts_with(&self.root) {
            return cwd;
        }

        let canonical_root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        let canonical_cwd = fs::canonicalize(&cwd).unwrap_or_else(|_| cwd.clone());
        canonical_cwd
            .strip_prefix(&canonical_root)
            .map(|relative| self.root.join(relative))
            .unwrap_or_else(|_| self.root.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::{Mutex, OnceLock};
    use std::time::Duration;

    use tempfile::tempdir;

    use super::WorkspaceMemory;
    use crate::prompt::PromptSourceKind;

    fn cwd_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    struct CurrentDirGuard {
        previous: PathBuf,
    }

    impl CurrentDirGuard {
        fn set(path: &Path) -> Self {
            let previous = std::env::current_dir().expect("current dir");
            std::env::set_current_dir(path).expect("set current dir");
            Self { previous }
        }
    }

    impl Drop for CurrentDirGuard {
        fn drop(&mut self) {
            let _ = std::env::set_current_dir(&self.previous);
        }
    }

    #[test]
    fn discover_prompt_sources_includes_nested_agents_files() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let nested = root.join("src/tools");
        fs::create_dir_all(&nested).expect("mkdir");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::write(root.join("AGENTS.md"), "root rules").expect("write root agents");
        fs::write(root.join("src").join("AGENTS.md"), "src rules").expect("write src agents");

        let workspace = WorkspaceMemory::from_paths(root.clone(), rara_dir);
        let sources = workspace.discover_prompt_sources_from_dir(&nested);

        let project_sources = sources
            .into_iter()
            .filter(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .collect::<Vec<_>>();
        assert_eq!(project_sources.len(), 2);
        assert_eq!(project_sources[0].display_path, "AGENTS.md");
        assert_eq!(project_sources[1].display_path, "src/AGENTS.md");
        assert_eq!(project_sources[0].content, "root rules");
        assert_eq!(project_sources[1].content, "src rules");
    }

    #[test]
    fn discover_prompt_sources_keeps_user_instruction_as_stable_prefix() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_home = dir.path().join("home").join(".rara");
        let rara_dir = rara_home.join("workspaces").join("repo-123");
        let nested = root.join("src/tools");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::create_dir_all(&rara_dir).expect("mkdir rara workspace");
        fs::write(rara_home.join("AGENTS.md"), "user rules").expect("write user agents");
        fs::write(root.join("AGENTS.md"), "root rules").expect("write root agents");
        fs::write(root.join("src").join("AGENTS.md"), "src rules").expect("write src agents");
        let user_instruction_path = rara_home.join("AGENTS.md").display().to_string();

        let workspace = WorkspaceMemory::from_paths(root.clone(), rara_dir);
        let sources = workspace.discover_prompt_sources_from_dir(&nested);

        let instruction_sources = sources
            .into_iter()
            .filter(|source| {
                matches!(
                    source.kind,
                    PromptSourceKind::UserInstruction | PromptSourceKind::ProjectInstruction
                )
            })
            .map(|source| (source.kind, source.display_path, source.content))
            .collect::<Vec<_>>();
        assert_eq!(
            instruction_sources,
            vec![
                (
                    PromptSourceKind::UserInstruction,
                    user_instruction_path,
                    "user rules".to_string()
                ),
                (
                    PromptSourceKind::ProjectInstruction,
                    "AGENTS.md".to_string(),
                    "root rules".to_string()
                ),
                (
                    PromptSourceKind::ProjectInstruction,
                    "src/AGENTS.md".to_string(),
                    "src rules".to_string()
                ),
            ]
        );
    }

    #[test]
    fn discover_prompt_sources_falls_back_to_root_when_cwd_is_outside_workspace() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(root.join("AGENTS.md"), "root rules").expect("write root agents");

        let workspace = WorkspaceMemory::from_paths(root.clone(), rara_dir);
        let sources = workspace.discover_prompt_sources_from_dir(&outside);

        let project_sources = sources
            .into_iter()
            .filter(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .collect::<Vec<_>>();
        assert!(project_sources.is_empty());
        let fallback_sources = workspace.discover_prompt_sources();
        let project_sources = fallback_sources
            .into_iter()
            .filter(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .collect::<Vec<_>>();
        assert_eq!(project_sources.len(), 1);
        assert_eq!(project_sources[0].display_path, "AGENTS.md");
    }

    #[test]
    fn read_git_branch_keeps_full_head_ref_name() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let git_dir = root.join(".git");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::create_dir_all(&git_dir).expect("mkdir git");
        fs::write(git_dir.join("HEAD"), "ref: refs/heads/feature/fix-issue\n").expect("write head");

        let workspace = WorkspaceMemory::from_paths(root, rara_dir);
        assert_eq!(workspace.read_git_branch(), "feature/fix-issue");
    }

    #[test]
    fn discover_prompt_sources_tracks_cwd_changes_inside_workspace() {
        let _lock = cwd_lock().lock().expect("cwd lock");
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let nested = root.join("src/tools");
        fs::create_dir_all(&nested).expect("mkdir nested");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::write(root.join("AGENTS.md"), "root rules").expect("write root agents");
        fs::write(root.join("src").join("AGENTS.md"), "src rules").expect("write src agents");
        let workspace = WorkspaceMemory::from_paths(root.clone(), rara_dir);

        let _guard = CurrentDirGuard::set(&nested);
        let nested_sources = workspace.discover_prompt_sources();
        let nested_project_sources = nested_sources
            .into_iter()
            .filter(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .map(|source| source.display_path)
            .collect::<Vec<_>>();
        assert_eq!(nested_project_sources, vec!["AGENTS.md", "src/AGENTS.md"]);
    }

    #[test]
    fn discover_prompt_sources_invalidates_modified_instruction_file() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        let agents = root.join("AGENTS.md");
        fs::write(&agents, "old rules").expect("write agents");
        let workspace = WorkspaceMemory::from_paths(root, rara_dir);

        let first_sources = workspace.discover_prompt_sources();
        let first = first_sources
            .iter()
            .find(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .expect("project instruction");
        assert_eq!(first.content, "old rules");

        std::thread::sleep(Duration::from_millis(20));
        fs::write(&agents, "new rules").expect("rewrite agents");

        let second_sources = workspace.discover_prompt_sources();
        let second = second_sources
            .iter()
            .find(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .expect("project instruction");
        assert_eq!(second.content, "new rules");
    }

    #[test]
    fn discover_prompt_sources_detects_memory_file_created_after_initial_miss() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        let workspace = WorkspaceMemory::from_paths(root, rara_dir.clone());

        let first_sources = workspace.discover_prompt_sources();
        assert!(
            first_sources
                .iter()
                .all(|source| !matches!(source.kind, PromptSourceKind::LocalMemory))
        );

        fs::write(rara_dir.join("memory.md"), "remember this").expect("write memory");

        let second_sources = workspace.discover_prompt_sources();
        let memory = second_sources
            .iter()
            .find(|source| matches!(source.kind, PromptSourceKind::LocalMemory))
            .expect("local memory source");
        assert_eq!(memory.content, "remember this");
    }

    #[test]
    fn discover_prompt_sources_falls_back_to_root_for_outside_cwd() {
        let _lock = cwd_lock().lock().expect("cwd lock");
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let outside = dir.path().join("outside");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::create_dir_all(&outside).expect("mkdir outside");
        fs::write(root.join("AGENTS.md"), "root rules").expect("write root agents");
        let workspace = WorkspaceMemory::from_paths(root, rara_dir);

        let _guard = CurrentDirGuard::set(&outside);
        let sources = workspace.discover_prompt_sources();
        let project_sources = sources
            .into_iter()
            .filter(|source| matches!(source.kind, PromptSourceKind::ProjectInstruction))
            .map(|source| source.display_path)
            .collect::<Vec<_>>();
        assert_eq!(project_sources, vec!["AGENTS.md"]);
    }

    #[test]
    fn get_env_info_invalidates_cached_branch_after_head_change() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        let git_dir = root.join(".git");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");
        fs::create_dir_all(&git_dir).expect("mkdir git");
        let head = git_dir.join("HEAD");
        fs::write(&head, "ref: refs/heads/main\n").expect("write head");

        let workspace = WorkspaceMemory::from_paths(root, rara_dir);
        let (_, branch) = workspace.get_env_info();
        assert_eq!(branch, "main");

        std::thread::sleep(Duration::from_millis(20));
        fs::write(&head, "ref: refs/heads/feature/runtime\n").expect("rewrite head");

        let (_, branch) = workspace.get_env_info();
        assert_eq!(branch, "feature/runtime");
    }

    #[test]
    fn has_memory_file_cached_tracks_write_and_read_paths() {
        let dir = tempdir().expect("tempdir");
        let root = dir.path().join("repo");
        let rara_dir = root.join(".rara");
        fs::create_dir_all(&rara_dir).expect("mkdir rara");

        let workspace = WorkspaceMemory::from_paths(root, rara_dir.clone());
        assert!(!workspace.has_memory_file_cached());

        workspace
            .write_memory_file("# Team Notes\n")
            .expect("write memory");
        assert!(workspace.has_memory_file_cached());

        fs::remove_file(rara_dir.join("memory.md")).expect("remove memory");
        assert!(workspace.has_memory_file_cached());

        assert_eq!(workspace.read_memory_file(), None);
        assert!(!workspace.has_memory_file_cached());
    }
}
