#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDefinition {
    /// Canonical name (also the file stem).
    pub name: String,
    /// Short description for /agents listing.
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
    pub token_budget: Option<i64>,
    /// Permission mode (e.g. "acceptEdits", "default").
    #[serde(default)]
    pub permission_mode: Option<String>,
    /// Whether plan approval is required before action.
    #[serde(default)]
    pub plan_mode_required: bool,
    /// Hidden from /agents listing (Claude Code compat).
    #[serde(default)]
    pub hidden: bool,
    /// System prompt — body after frontmatter `---`.
    #[serde(default, skip_deserializing)]
    pub system_prompt: String,
}

/// Loaded agent definition registry.
pub type AgentRegistry = HashMap<String, AgentDefinition>;

/// Load agent definitions from `.claude/agents/**/*.md`.
///
/// Each .md file must contain a YAML frontmatter block delimited by `---`.
/// The body after the closing `---` becomes `AgentDefinition::system_prompt`.
///
/// Built-in agents (general/explore/plan) are always available and do not
/// require a .claude/agents/ file.  Custom definitions can override built-in
/// names or define net new agents.
/// Load agent definitions from `.claude/agents/` in the workspace root
/// and `~/.claude/agents/*.md` (global config).  Workspace definitions
/// take precedence when names collide.
pub fn load_agent_definitions(workspace_root: &Path) -> AgentRegistry {
    let mut registry = AgentRegistry::new();

    // 1. Home-directory agents (lower precedence)
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        scan_agents_dir(&home.join(".claude").join("agents"), &mut registry);
    }

    // 2. Workspace agents (higher precedence — overwrites home)
    scan_agents_dir(
        &workspace_root.join(".claude").join("agents"),
        &mut registry,
    );

    registry
}

fn scan_agents_dir(agents_dir: &Path, registry: &mut AgentRegistry) {
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

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        if name.is_empty() {
            continue;
        }

        let (frontmatter, body) = split_frontmatter(&content);
        let mut def: AgentDefinition = match serde_yaml::from_str(&frontmatter) {
            Ok(d) => d,
            Err(_) => continue,
        };

        if def.name.is_empty() {
            def.name = name.clone();
        }

        // disallowedTools > tools
        if !def.disallowed_tools.is_empty() {
            def.tools.retain(|t| !def.disallowed_tools.contains(t));
        }

        def.system_prompt = body.trim().to_string();
        registry.insert(def.name.clone(), def);
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

