#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    /// Canonical name (also the file stem).
    #[serde(default)]
    pub name: String,
    /// Short description for agent listing and status surfaces.
    #[serde(default)]
    pub description: String,
    /// Allowed tools (Claude Code display names). Empty = all tools.
    #[serde(default)]
    pub tools: Vec<String>,
    /// Blocked tools. Takes precedence over `tools`.
    #[serde(default)]
    pub disallowed_tools: Vec<String>,
    /// Model override. None / "inherit" = use parent model.  
    /// If the specified model is unavailable, fall back to session default.
    #[serde(default)]
    pub model: Option<String>,
    /// Max tool-calling turns. 0 = system default.
    #[serde(default)]
    pub max_turns: usize,
    /// Positive token budget for spawned subagent execution.
    pub token_budget: Option<i64>,
    /// Claude-compatible permission mode override for spawned subagents.
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Whether plan approval is required before action.
    #[serde(default)]
    pub plan_mode_required: bool,
    /// Hidden from agent listing and status surfaces (Claude Code compat).
    #[serde(default)]
    pub hidden: bool,
    /// System prompt — body after frontmatter `---`.
    #[serde(default, skip_deserializing)]
    pub system_prompt: String,
}

/// Loaded agent definition registry.
pub type AgentRegistry = HashMap<String, AgentDefinition>;

#[derive(Clone, Debug)]
pub struct AgentDefinitionLoadRecord {
    pub id: String,
    pub source_path: PathBuf,
    pub definition: Option<AgentDefinition>,
    pub error: Option<String>,
}

#[derive(Clone, Debug)]
struct AgentDefinitionCacheState {
    registry: AgentRegistry,
    records: Vec<AgentDefinitionLoadRecord>,
}

/// Shared runtime cache for Claude-compatible agent definitions.
///
/// A cache is loaded once when the runtime agent is constructed. Rebuilding the
/// runtime constructs a new cache after editing `.rara/agents` or
/// `.claude/agents`.
#[derive(Clone, Debug)]
pub struct AgentDefinitionCache {
    state: Arc<AgentDefinitionCacheState>,
}

impl AgentDefinitionCache {
    pub fn load(workspace_root: impl Into<PathBuf>) -> Self {
        let workspace_root = workspace_root.into();
        let state = load_agent_definition_cache_state(&workspace_root);
        Self {
            state: Arc::new(state),
        }
    }

    pub fn resolve(&self, name: &str) -> Option<AgentDefinition> {
        resolve_agent(name, &self.state.registry)
    }

    pub fn records(&self) -> Vec<AgentDefinitionLoadRecord> {
        self.state.records.clone()
    }

    #[cfg(test)]
    pub fn from_records_for_test(records: Vec<AgentDefinitionLoadRecord>) -> Self {
        let mut registry = AgentRegistry::new();
        for record in &records {
            if let Some(definition) = &record.definition {
                registry.insert(definition.name.clone(), definition.clone());
            }
        }
        Self {
            state: Arc::new(AgentDefinitionCacheState { registry, records }),
        }
    }
}

fn load_agent_definition_cache_state(workspace_root: &Path) -> AgentDefinitionCacheState {
    let records = discover_agent_definition_records(workspace_root);
    let mut registry = AgentRegistry::new();
    for record in &records {
        match &record.definition {
            Some(definition) => {
                registry.insert(definition.name.clone(), definition.clone());
            }
            None => {
                log::warn!(
                    "failed to load agent definition {}: {}",
                    record.source_path.display(),
                    record.error.as_deref().unwrap_or("parse error")
                );
            }
        }
    }
    AgentDefinitionCacheState { registry, records }
}

pub fn discover_agent_definition_records(workspace_root: &Path) -> Vec<AgentDefinitionLoadRecord> {
    let mut records = Vec::new();
    for dir in agent_definition_dirs(workspace_root) {
        scan_agent_records_dir(&dir, &mut records);
    }
    records
}

#[cfg(test)]
pub fn discover_workspace_agent_definition_records(
    workspace_root: &Path,
) -> Vec<AgentDefinitionLoadRecord> {
    let mut records = Vec::new();
    for dir in workspace_agent_definition_dirs(workspace_root) {
        scan_agent_records_dir(&dir, &mut records);
    }
    records
}

fn agent_definition_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(home) = home_dir_from_env() {
        dirs.extend(agent_definition_dirs_for_root(&home));
    }
    dirs.extend(workspace_agent_definition_dirs(workspace_root));
    dirs
}

fn home_dir_from_env() -> Option<PathBuf> {
    home_dir_from_vars(std::env::var_os("HOME"), std::env::var_os("USERPROFILE"))
}

pub(super) fn home_dir_from_vars(
    home: Option<std::ffi::OsString>,
    userprofile: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    home.or(userprofile).map(PathBuf::from)
}

fn workspace_agent_definition_dirs(workspace_root: &Path) -> Vec<PathBuf> {
    agent_definition_dirs_for_root(workspace_root)
}

fn agent_definition_dirs_for_root(root: &Path) -> Vec<PathBuf> {
    vec![
        root.join(".claude").join("agents"),
        root.join(".rara").join("agents"),
    ]
}

fn scan_agent_records_dir(agents_dir: &Path, records: &mut Vec<AgentDefinitionLoadRecord>) {
    if !agents_dir.exists() || !agents_dir.is_dir() {
        return;
    }

    let walker = walkdir::WalkDir::new(agents_dir)
        .max_depth(4)
        .follow_links(false)
        .sort_by_file_name();

    for entry in walker.into_iter().filter_map(|e| e.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if path.extension().is_none_or(|e| e != "md") {
            continue;
        }

        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if id.is_empty() {
            continue;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(err) => {
                records.push(AgentDefinitionLoadRecord {
                    id,
                    source_path: path.to_path_buf(),
                    definition: None,
                    error: Some(format!("read error: {err}")),
                });
                continue;
            }
        };

        if content.trim().is_empty() {
            records.push(AgentDefinitionLoadRecord {
                id,
                source_path: path.to_path_buf(),
                definition: None,
                error: Some("empty file".to_string()),
            });
            continue;
        }

        let (frontmatter, body) = split_frontmatter(&content);
        let frontmatter_yaml = if frontmatter.trim().is_empty() {
            "{}"
        } else {
            frontmatter.as_str()
        };
        let mut def: AgentDefinition = match serde_yaml::from_str(frontmatter_yaml) {
            Ok(d) => d,
            Err(err) => {
                records.push(AgentDefinitionLoadRecord {
                    id,
                    source_path: path.to_path_buf(),
                    definition: None,
                    error: Some(format!("frontmatter parse error: {err}")),
                });
                continue;
            }
        };

        if def.name.is_empty() {
            def.name = id.clone();
        }

        // disallowedTools > tools
        if !def.disallowed_tools.is_empty() {
            def.tools.retain(|t| !def.disallowed_tools.contains(t));
        }

        def.system_prompt = body.trim().to_string();
        records.push(AgentDefinitionLoadRecord {
            id,
            source_path: path.to_path_buf(),
            definition: Some(def),
            error: None,
        });
    }
}

/// Split a .md file into (yaml_frontmatter, markdown_body).  
fn split_frontmatter(content: &str) -> (String, String) {
    let trimmed = content.trim_start();
    if !trimmed.starts_with("---") {
        return (String::new(), content.to_string());
    }
    let after_first = &trimmed[3..];
    if let Some(end) = after_first.find("\n---") {
        let yaml = after_first[..end].trim().to_string();
        let body = after_first[end + 4..].trim().to_string();
        (yaml, body)
    } else if let Some(end) = after_first.find("---") {
        let yaml = after_first[..end].trim().to_string();
        let body = after_first[end + 3..].trim().to_string();
        (yaml, body)
    } else {
        (String::new(), content.to_string())
    }
}

/// Resolve a named agent to its definition. Checks built-ins first, then registry.
pub fn resolve_agent(name: &str, registry: &AgentRegistry) -> Option<AgentDefinition> {
    match registry.get(name) {
        Some(d) => Some(d.clone()),
        None => builtin_agent_definition(name),
    }
}

fn builtin_agent_definition(name: &str) -> Option<AgentDefinition> {
    match name {
        "general" => Some(AgentDefinition {
            token_budget: None,
            name: "general".into(),
            description: "No-tool reasoning sub-agent".into(),
            tools: vec![],
            disallowed_tools: vec![],
            model: None,
            max_turns: 0,
            permission_mode: None,
            plan_mode_required: false,
            hidden: false,
            system_prompt: String::new(),
        }),
        "explore" => Some(AgentDefinition {
            token_budget: None,
            name: "explore".into(),
            description: "Read-only repository inspection sub-agent".into(),
            tools: vec!["Read".into(), "Glob".into(), "Grep".into()],
            disallowed_tools: vec!["Write".into(), "Edit".into(), "Bash".into()],
            model: None,
            max_turns: 50,
            permission_mode: None,
            plan_mode_required: false,
            hidden: false,
            system_prompt: String::new(),
        }),
        "plan" => Some(AgentDefinition {
            token_budget: None,
            name: "plan".into(),
            description: "Read-only planning sub-agent".into(),
            tools: vec!["Read".into(), "Glob".into(), "Grep".into()],
            disallowed_tools: vec!["Write".into(), "Edit".into(), "Bash".into()],
            model: None,
            max_turns: 30,
            permission_mode: None,
            plan_mode_required: true,
            hidden: false,
            system_prompt: String::new(),
        }),
        _ => None,
    }
}
