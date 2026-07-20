# Claude Plugin Runtime

## Problem

RARA needs to load and execute Claude Code plugins so that the growing ecosystem
of Claude Code plugins (memory, hooks, MCP launchers, etc.) can be reused
without forking. The plugin format includes metadata (`plugin.json`), hook
declarations (`hooks/hooks.json`), MCP server configurations (`.mcp.json`), and
optional commands/skills/agents directories. RARA already has a hook runtime
(`hook_runtime.rs`) and MCP client (`rara-mcp-client`), but neither understands
the plugin directory layout or the JSON-based hook declaration format.

## Scope

This spec covers:

- A new `rara-plugins` crate that owns plugin discovery, loading, and command
  hook execution.
- Parsing of `.claude-plugin/plugin.json` (name, version, description,
  configSchema, uiHints).
- Parsing of `hooks/hooks.json` into strongly-typed handler configurations with
  event binding (Stop, PreToolUse, PostToolUse, UserPromptSubmit, SessionStart,
  SessionEnd).
- Execution of `command`-type hooks: spawn a shell process, feed JSON on stdin,
  read exit code and stdout JSON (`{ continue: bool }`).
- Timeout enforcement per hook (default 60s, configurable).
- Integration with RARA's existing `HookRuntime` so plugin hooks fire on the
  same lifecycle events as built-in hooks.
- Plugin installation via `rara plugin install <source>` (git clone, local
  path, marketplace reference).

## Non-Goals

- No JavaScript engine (Bun, Deno, V8, QuickJS). Plugin hooks are executed as
  shell commands; plugin authors use whatever runtime they prefer (Node, Bun,
  Python, Bash, compiled binary).
- No `prompt`-type hook execution in the first slice. Prompt hooks inject LLM
  reasoning into the agent loop and require per-turn prompt assembly changes.
- No `http`-type hook execution (external webhook POST).
- No `agent`-type hook execution (sub-agent spawn for evaluation).
- No MCP server lifecycle management (start, stop, reconnect). The MCP config
  from `.mcp.json` is parsed but not yet launched.
- No configSchema-driven UI forms or CLI prompt flows.
- No plugin marketplace browsing from within RARA (just `install` from known
  sources).
- No plugin auto-update.

## Architecture

### Crate Boundary

```
crates/rara-plugins/
├── Cargo.toml
├── src/
│   ├── lib.rs              # re-exports
│   ├── loader.rs           # discover, load, parse plugin.json + hooks.json
│   ├── exec.rs             # execute_command_hook(async)
│   ├── types.rs            # Plugin, HookHandler, HookEvent, etc.
│   └── install.rs          # clone / copy / link plugin sources
```

Existing `src/plugin_loader.rs` and `src/plugin_exec.rs` will be deleted and
replaced by `crates/rara-plugins/`.

### Plugin Directory Layout

RARA will scan:

| Path | Priority | Content |
|---|---|---|
| `~/.rara/plugins/<name>/` | User | per-user plugins |
| `<workspace>/.rara/plugins/<name>/` | Project | per-project plugins |
| `--plugin-dir <path>` | CLI | manual override for TUI sessions |

A valid plugin directory contains:

```
<name>/
├── .claude-plugin/
│   └── plugin.json          # required
├── hooks/
│   └── hooks.json            # optional
├── .mcp.json                  # optional
├── commands/                  # optional
├── skills/                    # optional
└── agents/                    # optional
```

### Data Types

```rust
// types.rs

pub struct Plugin {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub root: PathBuf,
    pub source: PluginSource,  // User, Project, Cli
    pub hooks: Vec<RegisteredHook>,
    pub mcp_config: Option<McpConfig>,
    pub load_warnings: Vec<String>,
}

pub enum PluginSource {
    User(PathBuf),
    Project(PathBuf),
    Cli(PathBuf),
}

pub struct RegisteredHook {
    pub event: HookLifecycleKey,
    pub handler: HookHandler,
    pub plugin_name: String,
    pub plugin_root: PathBuf,
}

pub struct HookHandler {
    pub handler_type: HandlerType,
    pub command: String,
    pub timeout_secs: u64,
    pub matcher: Option<String>,
    pub once: bool,
}

pub enum HandlerType {
    Command,
    // future: Prompt, Http, Agent
}

/// Matches RARA's existing HookPhase enum in `hooks.rs`
pub enum HookLifecycleKey {
    Stop,
    PreToolUse,
    PostToolUse,
    UserPromptSubmit,
    SessionStart,
    SessionEnd,
}

pub struct McpConfig {
    pub servers: BTreeMap<String, McpServerDef>,
}
```

### Discovery And Loading

`PluginLoader::discover_all()` scans all three plugin directories, deduplicates
by name (user > project > cli — last write wins for same name), and returns a
flat `Vec<Plugin>`. Each plugin's `hooks/hooks.json` is parsed once at load
time.

### Command Hook Execution

`PluginRuntime::execute_hook(registered: &RegisteredHook, input: &HookInput) -> HookResult`

```
spawn("sh", ["-c", command])
  .env("CLAUDE_PLUGIN_ROOT", plugin_root)
  .stdin(piped)
  
stdin.write(json(input))
stdin.close()

wait_with_timeout(timeout_secs)
  → timed_out → HookResult::FailedTimeout
  → exit != 0 → HookResult::Failed(exit_code, stderr)
  → stdout has { "continue": false } → HookResult::Block(reason)
  → else → HookResult::Allow
```

### HookInput Payload

```json
{
  "session_id": "...",
  "transcript_path": "/path/to/session.jsonl",
  "hook_event": "Stop",
  "plugin_root": "/path/to/plugin",
  "tool_name": "Bash",        // only for PreToolUse/PostToolUse
  "tool_input": "..."         // only for PreToolUse/PostToolUse
}
```

### Integration With Existing Hook Runtime

RARA's `HookRuntime` already maps `AgentEvent` → `HookLifecycle` and runs a
dispatch loop. Plugin hooks are added as an additional source:

```rust
// In hook_runtime.rs or a new integration point:
let plugin_hooks = PluginLoader::load_all();
for hook in plugin_hooks {
    runtime.register(hook.event, hook);
}
```

When the hook runtime dispatches a lifecycle event, it iterates registered
hooks and calls `PluginRuntime::execute_hook()` for each `HandlerType::Command`
hook.

### Installation

```bash
rara plugin install https://github.com/clawmem-ai/clawmem-claude-code-plugin
rara plugin install ./local-plugin
rara plugin list
rara plugin remove clawmem
```

Installation copies/clones the plugin to `~/.rara/plugins/<name>/`. For git
URLs, a shallow clone is performed and kept for future updates.

## Contracts

### Crate API

`crates/rara-plugins` exposes:

```rust
pub fn discover_plugins(user_dir: &Path, project_dir: Option<&Path>) -> Vec<Plugin>;
pub fn load_plugin(root: &Path, source: PluginSource) -> Option<Plugin>;
pub async fn execute_command_hook(handler: &HookHandler, root: &Path, input: &HookInput) -> HookResult;
pub fn install_plugin(source: &str, target_dir: &Path) -> Result<Plugin>;
pub fn remove_plugin(name: &str, target_dir: &Path) -> Result<()>;
```

### File Format Compatibility

`hooks/hooks.json` format must match the Claude Code schema exactly:

```json
{
  "Stop": [{ "hooks": [{ "type": "command", "command": "...", "timeout": 20 }] }]
}
```

Unknown fields are ignored. Unknown event keys are ignored with a warning.
Matchers are parsed but not evaluated in the first slice (matching is deferred).

## Validation Matrix

| What | How |
|---|---|
| Plugin loads from valid directory | unit test with temp dir + plugin.json + hooks.json |
| Missing `.claude-plugin/` returns None | unit test |
| Invalid JSON produces load_warnings | unit test |
| `execute_command_hook` with working command | integration test (echo return code) |
| `{ "continue": false }` blocks | integration test |
| Non-zero exit code fails | integration test |
| Timeout fires | integration test (sleep 10) |
| `discover_plugins` skips non-directories | unit test |
| `discover_plugins` de-dupes by name | unit test |

## Implementation Status

Implemented in the first merged slice:

- `crates/rara-plugins` exists with loader, executor, and shared types.
- `.claude-plugin/plugin.json` and `hooks/hooks.json` are parsed.
- Command hooks can be executed with stdin JSON, timeout handling, exit-code
  reporting, and stdout parsing.
- A middleware bridge exists in `src/plugin_middleware.rs` and is used by the
  TUI runtime rebuild path for workspace plugin hook registration.
- The discovery API can scan a single directory with source metadata or scan
  multiple ordered sources with name-based de-duplication. Later sources in the
  ordered list override earlier sources, so callers own their precedence policy.
- Workspace plugin CLI uses the project source for
  `<workspace>/.rara/plugins`.
- TUI runtime hook registration combines `~/.rara/plugins` as the user source
  and `<workspace>/.rara/plugins` as the project source through the ordered
  discovery API. Project plugins override user plugins with the same plugin
  name.
- User plugin home resolution happens on the blocking registration worker. If
  user plugin home cannot be resolved, project plugin registration still runs.
- TUI and `resume` startup accept plugin directories from persisted
  `plugin_dirs` config and repeated `--plugin-dir <path>` global CLI flags.
  Configured directories are appended before CLI directories, and both are
  passed into plugin hook registration as the final explicit source tier. CLI
  plugin directories therefore override configured explicit directories, project
  plugins, and user plugins with the same plugin name. Relative explicit plugin
  directories are normalized to absolute paths during CLI startup. Supplying any
  explicit plugin directories triggers the TUI runtime rebuild path on startup
  so the hook runtime is created and plugin hooks are registered even when local
  embedding startup is disabled.

Next implementation slices:

1. Extend plugin source composition beyond the TUI rebuild path to headless,
   ACP, and Wire runtime startup.
2. Fix lifecycle parity gaps before broad user-facing rollout: `SessionEnd` mapping,
   matcher evaluation, blocking hook results, and hook output observability.
3. Add git-source install support on top of the existing local-directory
   `rara plugin install/list/remove` commands.
4. Feed plugin `.mcp.json`, commands, skills, and agents into the same
   structured extension-source registries used by native RARA features.

## Open Risks

- Hook scripts may `require` Node modules that aren't installed in the user's
  environment. RARA cannot manage Node dependencies. Plugin `README.md` must
  document runtime requirements.
- Matcher pattern evaluation (`Bash(*)`, `Write|Edit`) is deferred. All hooks
  currently fire for all tool calls within their event.
- `prompt`-type hooks require prompt assembly integration that is not yet
  designed.
- No sandboxing of hook processes. A malicious plugin hook script has full user
  access. Trust is on the user who installed the plugin.

## Source Journals

- `docs/journal/2026-05-12-claude-plugin-runtime.md`
- `docs/journal/2026-05-12-main-sync-development-plan.md`
- `docs/journal/2026-07-19-plugin-source-discovery.md`
- `docs/journal/2026-07-20-plugin-dir-config.md`
