use std::fs;
use std::path::Path;

use rara_plugins::{PluginDiscoverySource, PluginSource};

use crate::config::NowledgeMemPluginConfig;

pub(super) const BUILTIN_PLUGINS_DIR: &str = "builtin-plugins";
pub(super) const NOWLEDGE_MEM_PLUGIN_DIR: &str = "nowledge-mem";
#[cfg(test)]
pub(super) const NOWLEDGE_MEM_MCP_URL: &str = "http://127.0.0.1:14242/mcp/";
#[cfg(test)]
pub(super) const NOWLEDGE_MEM_CLOUD_MCP_URL: &str = "https://cloud.nowledge.co/remote-api/mcp/";

const NOWLEDGE_MEM_PLUGIN_VERSION: &str = "0.1.29-rara.1";
pub(crate) const NOWLEDGE_MEM_PROMPT_INSTRUCTIONS: &str = r#"## Nowledge Mem Context

When Nowledge Mem is available, load the Context Bundle at session start when
identity, workspace scope, rules, or prior decisions may affect the task. Use
Working Memory for the current briefing and search exact memories or threads
when needed. After context compaction, refresh the Context Bundle or Working
Memory before continuing so important constraints are not lost. Memory is a
context aid: continue with a runtime warning if the service is unavailable."#;
const NOWLEDGE_MEM_SKILL_ROOTS: &[(&str, &str)] = &[
    ("working-memory", NOWLEDGE_MEM_WORKING_MEMORY_SKILL),
    ("search-memory", NOWLEDGE_MEM_SEARCH_MEMORY_SKILL),
    ("distill-memory", NOWLEDGE_MEM_DISTILL_MEMORY_SKILL),
    ("save-thread", NOWLEDGE_MEM_SAVE_THREAD_SKILL),
    ("status", NOWLEDGE_MEM_STATUS_SKILL),
];

pub(super) fn discovery_sources(
    rara_home: &Path,
    config: &NowledgeMemPluginConfig,
) -> Vec<PluginDiscoverySource> {
    if !config.enabled {
        return Vec::new();
    }
    let plugins_dir = rara_home.join(BUILTIN_PLUGINS_DIR);
    let plugin_root = plugins_dir.join(NOWLEDGE_MEM_PLUGIN_DIR);
    if let Err(err) = materialize_nowledge_mem_plugin(&plugin_root, config) {
        log::warn!(
            "failed to materialize builtin Nowledge Mem plugin at {}: {err}",
            plugin_root.display()
        );
        return Vec::new();
    }
    vec![PluginDiscoverySource {
        plugins_dir: plugins_dir.clone(),
        source: PluginSource::Builtin(plugins_dir),
    }]
}

fn materialize_nowledge_mem_plugin(
    plugin_root: &Path,
    config: &NowledgeMemPluginConfig,
) -> std::io::Result<()> {
    write_file_if_changed(
        &plugin_root.join(".codex-plugin").join("plugin.json"),
        &serde_json::json!({
            "name": NOWLEDGE_MEM_PLUGIN_DIR,
            "version": NOWLEDGE_MEM_PLUGIN_VERSION,
            "description": "Nowledge Mem integration for RARA runtime memory, cross-tool recall, and thread handoff.",
            "skills": "./skills/",
            "mcpServers": "./.mcp.json"
        })
        .to_string(),
    )?;
    write_file_if_changed(
        &plugin_root.join(".mcp.json"),
        &serde_json::json!({
            "mcpServers": {
                "nowledge-mem": {
                    "type": "http",
                    "url": config.mcp_url(),
                    "http_headers": nowledge_mem_http_headers(config),
                    "env_http_headers": config.env_http_headers()
                }
            }
        })
        .to_string(),
    )?;
    write_file_if_changed(&plugin_root.join("AGENTS.md"), NOWLEDGE_MEM_AGENTS_MD)?;
    for (name, content) in NOWLEDGE_MEM_SKILL_ROOTS {
        write_file_if_changed(
            &plugin_root.join("skills").join(name).join("SKILL.md"),
            content,
        )?;
    }
    write_file_if_changed(
        &plugin_root.join("agents").join("nowledge-mem.md"),
        NOWLEDGE_MEM_AGENT_DEFINITION,
    )
}

fn nowledge_mem_http_headers(
    config: &NowledgeMemPluginConfig,
) -> std::collections::BTreeMap<String, String> {
    let mut headers = std::collections::BTreeMap::from([("APP".to_string(), "RARA".to_string())]);
    headers.extend(config.http_headers.clone());
    headers
}

fn write_file_if_changed(path: &Path, content: &str) -> std::io::Result<()> {
    if fs::read_to_string(path).is_ok_and(|current| current == content) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)
}

const NOWLEDGE_MEM_AGENTS_MD: &str = r#"# Nowledge Mem

Nowledge Mem is the source for cross-tool context, current Working Memory, exact
prior threads, and sourced decisions. RARA local memory remains available for
local workspace recall, but do not treat it as a replacement for Nowledge Mem
when the user asks about prior cross-tool context or exact history.

Prefer the Nowledge Mem MCP server when it is reachable. Use the plugin skills
as routing guidance and CLI fallback procedures when MCP tools are not exposed
in the current session.

The same lifecycle guidance is included in RARA's default prompt when this
builtin plugin is enabled.
"#;

const NOWLEDGE_MEM_AGENT_DEFINITION: &str = r#"---
name: nowledge-mem
description: Routes memory-heavy tasks through Nowledge Mem skills and MCP context.
tools: []
---

You are the Nowledge Mem routing subagent for RARA.

Use this role when the parent agent asks how to retrieve, save, or verify
cross-tool memory. Inspect the Nowledge Mem skill descriptions available in the
session prompt and return the exact memory route the parent should use.

You do not call external tools directly from this subagent role. The parent
agent owns MCP, shell, and skill invocation decisions for the active session.
"#;

const NOWLEDGE_MEM_WORKING_MEMORY_SKILL: &str = r#"---
name: working-memory
description: Load the Nowledge Mem working context or daily briefing before context-sensitive work.
---

# Working Memory

Use this skill when a task depends on current priorities, prior decisions,
active project context, or cross-tool continuity.

Prefer Nowledge Mem MCP context tools when they are exposed in the session. If
MCP is not available, use:

```bash
nmem --json context --source-app rara
```

For a lighter daily briefing, use:

```bash
nmem --json wm read
```

Keep the returned context as sourced task context. Do not copy large memory
payloads into durable RARA prompts unless the user explicitly asks for a
summary or handoff.
"#;

const NOWLEDGE_MEM_SEARCH_MEMORY_SKILL: &str = r#"---
name: search-memory
description: Search Nowledge Mem for durable decisions, prior threads, or exact cross-tool history.
---

# Search Memory

Use this skill before answering continuation, review, regression, release,
connector, prior-decision, or exact-history questions.

Prefer Nowledge Mem MCP search tools when they are available. If MCP is not
available, start with durable memory search:

```bash
nmem --json m search "query"
```

Use thread search when the user asks about a previous conversation or exact
session history:

```bash
nmem --json t search "query" --limit 5
nmem --json t show <thread_id> --limit 8 --offset 0 --content-limit 1200
```

Treat Codex or RARA local memory as hints. Nowledge Mem remains the authority
for cross-tool state and sourced decisions.
"#;

const NOWLEDGE_MEM_DISTILL_MEMORY_SKILL: &str = r#"---
name: distill-memory
description: Save durable cross-tool or cross-workspace decisions, procedures, and reusable knowledge to Nowledge Mem, the durable/semantic memory authority. Prefer this over RARA's workspace-local memory.md for knowledge that must outlive the workspace.
---

# Distill Memory

Use this skill when the user asks to remember something, save a durable
decision, or preserve a reusable procedure across tools.

Nowledge Mem is the authority for durable, cross-tool, and cross-workspace
knowledge. RARA's local `memory.md` and `MemoryRecord` files are workspace-local,
short-term, plain-text substrate. Write durable or cross-tool knowledge here
instead of local memory, and do not persist the same fact into both stores.

Search first to avoid duplicates:

```bash
nmem --json m search "concept"
```

If an existing memory should change, update it:

```bash
nmem --json m update <memory_id> -c "updated content"
```

If this is new durable knowledge, add it:

```bash
nmem --json m add "content" -t "Title" --unit-type decision -l "label" -s rara -i 0.8
```

Keep memory entries concise, sourced by the current task, and focused on
knowledge that should survive the current thread.
"#;

const NOWLEDGE_MEM_SAVE_THREAD_SKILL: &str = r#"---
name: save-thread
description: Save or hand off the current RARA thread into Nowledge Mem when the user asks for a durable thread record.
---

# Save Thread

Use this skill when the user asks to save the current thread, create a handoff,
or preserve exact conversation context for another tool.

Prefer a real transcript import when the host exposes one. If the only
available route is the Nowledge Mem CLI, use:

```bash
nmem --json t save --from rara -p . -s "Brief summary"
```

Do not fabricate a transcript. If the host cannot expose the real session
transcript, save a concise handoff summary only after the user explicitly asks
for it.
"#;

const NOWLEDGE_MEM_STATUS_SKILL: &str = r#"---
name: status
description: Check whether Nowledge Mem is reachable through MCP or the nmem CLI.
---

# Status

Use this skill when memory tools are missing, MCP search fails, or the user asks
whether Nowledge Mem is connected.

Prefer MCP status tools if exposed. For CLI fallback, run:

```bash
nmem --json status
```

If the local MCP server is unavailable, verify that Nowledge Mem is running and
that the local endpoint is available at `http://127.0.0.1:14242/mcp/`.
"#;
