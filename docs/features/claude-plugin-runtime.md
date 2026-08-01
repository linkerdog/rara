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
- Parsing of `.claude-plugin/plugin.json` and compatible
  `.codex-plugin/plugin.json` metadata (name, version, description,
  configSchema, uiHints).
- Parsing of `hooks/hooks.json` into strongly-typed handler configurations with
  event binding (Stop, PreToolUse, PostToolUse, UserPromptSubmit, SessionStart,
  SessionEnd).
- Execution of `command`-type hooks: spawn a shell process, feed JSON on stdin,
  read exit code and stdout JSON (`{ continue: bool }`).
- Synchronous `PreToolUse` command-hook blocking in the agent tool execution
  path.
- Direct `SessionStart` and `UserPromptSubmit` command hooks from the agent
  query path.
- `SessionEnd` command hooks on final agent-loop completion.
- Prompt-visible summaries for plugin `skills/<name>/SKILL.md` directories.
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
- No plugin-owned MCP lifecycle manager. Plugin `.mcp.json` files are registered
  into RARA's shared MCP registry; connection, refresh, status, and tool-cache
  behavior remain owned by the existing MCP runtime path.
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
| `~/.rara/builtin-plugins/<name>/` | Builtin | RARA-owned compatibility plugins |
| `~/.rara/plugins/<name>/` | User | per-user plugins |
| `<workspace>/.rara/plugins/<name>/` | Project | per-project plugins |
| `--plugin-dir <path>` | CLI | manual override for TUI sessions |

A valid plugin directory contains:

```
<name>/
├── .claude-plugin/           # or .codex-plugin/
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
    Builtin(PathBuf),
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
  "tool_name": "Bash",        // only for tool events
  "tool_input": { "command": "git status" },
  "tool_response": null,      // reserved for PostToolUse
  "last_assistant_message": null,
  "is_interrupt": null,
  "prompt": null              // only populated for UserPromptSubmit
}
```

### Integration With Existing Hook Runtime

Plugin discovery and registration are owned by runtime bootstrap. The resulting
plugin hook set is session-scoped and attached to the agent alongside the
session-owned `HookRuntime`.

`PreToolUse` command hooks are executed synchronously at the tool execution
boundary. A hook result with `continue:false`, a non-zero exit, or a timeout
returns an error `tool_result` to the model and the tool is not executed. The
blocking message is selected from stdout JSON `stopReason`, `reason`, or
`systemMessage`, then stderr, plain stdout, then a generic fallback. `PreToolUse`
stdout is treated as control output for the blocking decision and is not
injected into later model context.

Non-blocking plugin events continue to register with `HookRuntime` and inject
stdout into the model context before a later model turn. `PreToolUse` is not
also registered through the async event-bus callback, so command hooks are not
run twice.

`SessionStart` and `UserPromptSubmit` command hooks are executed directly by
the agent query path instead of through the async event-bus callback.
`SessionStart` fires once for each `Agent` instance before the first submitted
prompt is compacted or appended to history. `UserPromptSubmit` fires once for
each `query_with_mode_and_events` call and receives the submitted prompt in the
`prompt` field. These lifecycle hooks do not block the turn. Their command
stdout, stderr, exit code, timeout state, and success state are published as
structured `RuntimeEvent::Hook(command_output)` control-plane events and are not
injected into model context.

`SessionEnd` command hooks are not registered through the async event-bus
callback because there is no ordinary tool event to translate. The agent loop
executes them directly when the loop reaches a terminal completion or hard stop
such as max-turn or token-budget exhaustion. Waiting states such as shell
approval or plan-exit approval do not fire `SessionEnd` because the session is
paused rather than complete. The hook input includes `last_assistant_message`
when a final assistant message is available. Normal completion sends
`is_interrupt: false`; model-turn cancellation sends `is_interrupt: true`
before returning the cancellation error to the caller. `SessionEnd` hook
failures are logged and do not block completion; stdout observability is
published through the same structured hook command-output event used by other
non-blocking lifecycle hooks.

### Installation

```bash
rara plugin install https://github.com/clawmem-ai/clawmem-claude-code-plugin
rara plugin install ./local-plugin
rara plugin list
rara plugin remove clawmem
```

Installation copies the plugin into the current workspace's
`.rara/plugins/<name>/` directory. Local directory sources are copied directly.
Git sources (`https://`, `ssh://`, `git://`, `file://`, and `git@...`) are
shallow-cloned to a temporary checkout, validated as Claude Code plugin
directories, copied under the plugin name from `.claude-plugin/plugin.json`,
and then removed from the temporary checkout location. Existing plugins require
`--force` to replace.

### Skill Extension Registry

Runtime bootstrap discovers `skills/<name>/SKILL.md` directories from loaded
plugins and loads them into the shared `SkillManager`. Plugin skill names are
exposed as `plugin_name:skill_name`, use `scope: "plugin"`, and are available
through the existing `skill` tool `list`, `invoke`, and `reload` actions.

Plugin skill reload uses the same runtime-owned plugin roots captured during
bootstrap. Reload updates the running `SkillManager`; it is not a validation-only
rescan.

### MCP Extension Registry

Runtime bootstrap registers plugin `.mcp.json` files into the same source-aware
`McpRegistry` used by user `config.toml` and workspace `.mcp.json` files.
Plugin MCP entries use `scope: "plugin"` and keep the source path at the plugin
`.mcp.json` file so `/mcp`, runtime-control events, ACP, Wire, and future
app-server clients can display provenance without parsing plugin directories
themselves.

Plugin MCP server names do not override user or workspace MCP servers. A
duplicate server name fails registry assembly with both source paths, matching
the base MCP registry contract. Stdio plugin MCP servers receive `cwd` set to
the plugin root when the existing MCP runtime later connects them.

MCP JSON parsing accepts Codex-style HTTP entries with an informational
`"type": "http"` field and maps them onto RARA's existing streamable HTTP
transport. RARA does not run a plugin-specific MCP lifecycle manager; it only
registers server definitions into the shared runtime registry.

### Command Extension Registry

Runtime bootstrap discovers plugin `commands/**/*.md` files from loaded
plugins and attaches compact summaries to the session plugin runtime. Plugin
command names are exposed as `plugin_name:command_name`, with nested command
paths preserved as `/` separators. The command description comes from leading
frontmatter `description` when present, otherwise from the first non-heading
body line.

Plugin command summaries are runtime-owned metadata only. TUI and protocol
surfaces may render the command count or future command details from the
runtime snapshot, but plugin commands are not routed through the TUI local
slash-command parser and are not executable until a shared command invocation
contract exists.

### Agent Extension Registry

Runtime bootstrap discovers plugin `agents/**/*.md` definitions and feeds them
into the same session-scoped `AgentDefinitionCache` used by workspace and user
agent definitions. Plugin agent names are exposed as `plugin_name:agent_name`,
which avoids collisions with built-in, user, and workspace agents.

Plugin agent definitions use the existing Claude-compatible agent frontmatter
contract: `description`, `tools`, `disallowed_tools`, `model`, `max_turns`,
`token_budget`, `permission_mode`, `plan_mode_required`, and `hidden`.

### Builtin Nowledge Mem Plugin

RARA materializes a builtin `nowledge-mem` plugin under
`~/.rara/builtin-plugins/nowledge-mem` during runtime plugin discovery. This
plugin is based on the Nowledge Mem community Codex plugin shape, but is kept as
a compact RARA-owned compatibility package instead of vendoring the whole
community repository.

The builtin plugin provides:

- `.codex-plugin/plugin.json` metadata so the same loader path accepts Codex
  plugin manifests.
- A `nowledge-mem` streamable HTTP MCP server pointing at
  `http://127.0.0.1:14242/mcp/` with `APP: RARA`.
- Plugin skills for working memory, memory search, memory distillation, thread
  save guidance, and status diagnostics.
- A `nowledge-mem:nowledge-mem` plugin agent definition for subagent routing
  guidance.

The builtin plugin is configurable through `config.json`:

```json
{
  "builtin_plugins": {
    "nowledge_mem": {
      "enabled": true,
      "mode": "local",
      "url": "http://127.0.0.1:14242/mcp/",
      "http_headers": {
        "APP": "RARA"
      }
    }
  }
}
```

Default values are omitted when config is serialized. `enabled: false` disables
materialization and discovery of the builtin Nowledge Mem plugin. `url` and
`http_headers` override the generated builtin `.mcp.json`; custom headers merge
with the default `APP: RARA` header and may replace it.

Builtin plugins are the lowest-precedence source. User, project, and explicit
CLI plugins with the same plugin name override the builtin plugin through the
normal ordered de-duplication path. Builtin MCP servers also yield to already
registered user or project MCP servers with the same name; normal non-builtin
plugin MCP duplicates remain hard errors.

Cloud mode is also supported. Cloud mode defaults to the fixed Nowledge Mem
server `https://cloud.nowledge.co`. The generated endpoint is
`/remote-api/mcp/`. The transport emits
Authorization and X-NMEM-API-Key from the configured API key, plus the optional
X-Nmem-Space-Id from NMEM_SPACE. The key is persisted using RARA's existing
secret configuration field and is exposed to the runtime only through
NMEM_API_KEY; the generated plugin file contains only the environment variable
reference. The environment variable names can be configured with
api_key_env_var and space_id_env_var.

`rara mem --api-key <key>` saves the key to RARA's configuration and applies it
to subsequent runs, including after restart. The user does not need to
configure `NMEM_API_KEY` separately.

Local Nowledge Mem endpoints must not use system HTTP proxies. The MCP
transport contract exposes localhost proxy bypass detection for streamable HTTP
URLs whose host is `localhost`, `127.0.0.1`, or `::1`. Any future streamable
HTTP MCP connector must call this helper before constructing its HTTP client.

The builtin subagent does not receive additional external execution authority.
It is a registry-provided routing agent that can explain which Nowledge Mem
skill, MCP tool, or CLI fallback the parent runtime should use. Every child
runtime receives a default-deny `SubagentPluginCapabilityPolicy`; direct MCP,
shell, or skill invocation inside subagents requires a later scoped executor
and explicit capability allowlist.

TUI displays this integration in the `/status` Overview Extensions section.
The `/mem` configuration command opens a picker for Disabled, Local, or Cloud;
it does not accept command arguments. Cloud configuration accepts the server
URL and environment variable names only; it never accepts an API-key value.
Saving a mode choice persists the config and asks the runtime to rebuild. The
TUI does not assemble the MCP transport itself.
The status display reports the builtin MCP entry as
`nowledge-mem builtin`, shows the configured endpoint with secret-bearing URL
parts redacted, and marks localhost endpoints as `local/direct` to make the
no-proxy contract visible. Disabled builtin configuration renders as
`nowledge-mem disabled`. TUI does not own plugin discovery, MCP registration,
or generated plugin files.

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
Tool matchers are evaluated by tool name, including simple `A|B`, `A,B`, and
`Tool(...)` forms. Full Claude Code input-pattern matching remains out of
scope for now.

## Validation Matrix

| What | How |
|---|---|
| Plugin loads from valid directory | unit test with temp dir + plugin.json + hooks.json |
| Missing `.claude-plugin/` returns None | unit test |
| Invalid JSON produces load_warnings | unit test |
| `execute_command_hook` with working command | integration test (echo return code) |
| `{ "continue": false }` blocks command hook execution | `cargo test agent::tests::plugin_hooks::plugin_pre_tool_use_continue_false_blocks_tool_execution -- --nocapture` |
| `SessionEnd` receives final assistant payload | `cargo test agent::tests::plugin_hooks::plugin_session_end_runs_once_with_last_assistant_message -- --nocapture` |
| `SessionEnd` marks cancelled model turns as interrupts | `cargo test agent::tests::plugin_hooks::plugin_session_end_marks_cancelled_model_turn_as_interrupt -- --nocapture` |
| `SessionStart` and `UserPromptSubmit` fire from agent queries | `cargo test agent::tests::plugin_hooks::plugin_non_tool_lifecycle_hooks_run_from_agent_query -- --nocapture` |
| Lifecycle hook stdout/stderr publishes a structured control event | `cargo test plugin_middleware::tests::lifecycle_hook_output_is_published_as_structured_control_event -- --nocapture` and `cargo test runtime_control::tests::hook_command_output_uses_structured_wire_shape -- --nocapture` |
| Plugin skills load into the shared skill registry | `cargo test -p rara-skills plugin_skills_are_namespaced_and_invokable -- --nocapture` and `cargo test tools::skill::reload_updates_running_manager_with_plugin_skills -- --nocapture` |
| Plugin `.mcp.json` registers into the shared MCP registry | `cargo test plugin_middleware::tests::appends_plugin_mcp_configs_with_plugin_source_metadata -- --nocapture` |
| Codex-style HTTP MCP JSON parses | `cargo test -p rara-config loads_codex_style_http_mcp_json_type_field -- --nocapture` |
| Builtin Nowledge Mem plugin materializes skills, MCP, and agent definition | `cargo test plugin_middleware::tests::builtin_nowledge_mem_plugin_materializes_skills_mcp_and_agent -- --nocapture` |
| Builtin Nowledge Mem MCP registers as builtin fallback | `cargo test plugin_middleware::tests::appends_builtin_nowledge_mem_mcp_config -- --nocapture` and `cargo test plugin_middleware::tests::builtin_nowledge_mem_mcp_yields_to_existing_registry_server -- --nocapture` |
| Builtin Nowledge Mem config controls endpoint, headers, and enabled state | `cargo test plugin_middleware::tests::builtin_nowledge_mem_mcp_uses_configured_url_and_headers -- --nocapture`, `cargo test plugin_middleware::tests::disabled_builtin_nowledge_mem_plugin_is_not_discovered -- --nocapture`, and `cargo test -p rara-config builtin_nowledge_mem_config_can_override_endpoint_and_headers -- --nocapture` |
| TUI configures and shows builtin Nowledge Mem without owning runtime assembly | `cargo test tui::command::tests::parses_nowledge_mem_configuration_command -- --nocapture`, `cargo test tui::status_display::tests::overview_status_reports_builtin_nowledge_mem -- --nocapture`, `cargo test tui::status_display::tests::overview_status_reports_disabled_nowledge_mem -- --nocapture`, and `cargo test tui::status_display::tests::overview_status_reports_custom_nowledge_mem_endpoint_and_headers -- --nocapture` |
| Local streamable HTTP MCP endpoints bypass proxy | `cargo test -p rara-config streamable_http_localhost_bypasses_proxy -- --nocapture` |
| Plugin MCP file and relative cwd handling | `cargo test plugin_middleware::tests::plugin_mcp_configs_skip_mcp_json_directories -- --nocapture` and `cargo test plugin_middleware::tests::plugin_mcp_configs_resolve_relative_cwd_from_plugin_root -- --nocapture` |
| Plugin MCP parse and duplicate-name failures surface | `cargo test plugin_middleware::tests::plugin_mcp_configs_fail_on_duplicate_server_names -- --nocapture` and `cargo test plugin_middleware::tests::plugin_mcp_configs_fail_on_invalid_json -- --nocapture` |
| Plugin command markdown files register as runtime summaries | `cargo test plugin_middleware::tests::registers_project_plugin_command_summaries -- --nocapture` |
| Plugin agent markdown files register as runtime agent definitions | `cargo test plugin_middleware::tests::plugin_agent_records_are_namespaced_by_plugin_name -- --nocapture` |
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
- `PreToolUse` command hooks run synchronously before tool execution. Blocking
  results return an error tool result to the model and skip the tool call.
- A middleware bridge exists in `src/plugin_middleware.rs` and is owned by
  runtime bootstrap assembly rather than by any presentation surface.
- The discovery API can scan a single directory with source metadata or scan
  multiple ordered sources with name-based de-duplication. Later sources in the
  ordered list override earlier sources, so callers own their precedence policy.
- Workspace plugin CLI uses the project source for
  `<workspace>/.rara/plugins`.
- Runtime hook registration combines `~/.rara/plugins` as the user source and
  `<workspace>/.rara/plugins` as the project source through the ordered
  discovery API. Project plugins override user plugins with the same plugin
  name. The same bootstrap path is used by TUI, ask, print, headless exec,
  ACP, and Wire surfaces; those surfaces pass runtime options and then render or
  translate the resulting event stream.
- User plugin home resolution happens on the blocking registration worker. If
  user plugin home cannot be resolved, project plugin registration still runs.
- TUI, `resume`, ask, print, headless exec, ACP, and Wire startup accept plugin
  directories from persisted `plugin_dirs` config and repeated
  `--plugin-dir <path>` global CLI flags.
- Plugin `.mcp.json` definitions are converted into the shared MCP registry
  with `plugin` scope, plugin-root `cwd` for stdio servers, and duplicate-name
  conflict handling shared with user and project MCP sources.
  Configured directories are appended before CLI directories, and both are
  passed into plugin hook registration as the final explicit source tier. CLI
  plugin directories therefore override configured explicit directories, project
  plugins, and user plugins with the same plugin name. Relative explicit plugin
  directories are normalized to absolute paths during CLI startup, and duplicate
  normalized directories are scanned only once. Runtime bootstrap creates and
  starts a session-scoped hook runtime, registers plugin command hooks, and
  attaches that hook runtime to the agent before handing the agent to the
  surface consumer. Presentation surfaces must not route plugin behavior
  through TUI-owned state or process-global strong references.
- `hooks/hooks.json` matcher groups are preserved on registered hook handlers.
  Tool hook matchers are evaluated before command execution. Empty matchers and
  `*` match all tools; exact tool names are matched case-insensitively; Claude
  Code-style tool patterns such as `Bash(*)` match by the tool name before the
  parenthesized input pattern; alternatives can be separated with `|` or `,`.
- `SessionEnd` command hooks execute once when the agent loop reaches final
  completion or a hard stop. They receive `hook_event: "SessionEnd"`, empty
  tool fields, the best available `last_assistant_message`, and
  `is_interrupt: false`. Cancelled model turns fire `SessionEnd` with
  `is_interrupt: true` before returning the cancellation error. Approval waits
  and other resumable pauses do not fire `SessionEnd`.
- `SessionStart` and `UserPromptSubmit` command hooks execute directly from
  agent queries. `SessionStart` runs once per agent instance. `UserPromptSubmit`
  runs once per submitted query and includes the submitted prompt. Both
  lifecycle hooks are non-blocking.
- Non-blocking lifecycle hook stdout, stderr, exit code, timeout state, and
  success state are published as `RuntimeEvent::Hook(command_output)` events on
  the session control bus. These events are structured observability only and do
  not feed model context.
- Plugin `skills/<name>/SKILL.md` directories are loaded into the shared
  `SkillManager` with namespaced `plugin_name:skill_name` names and plugin
  scope. The `skill` tool can list, invoke, and reload them.
- Plugin `commands/**/*.md` files are exposed as runtime-owned command
  summaries with namespaced `plugin_name:command_name` names. `/status`
  displays the loaded command count from the runtime snapshot. Invocation is
  intentionally deferred until a shared command execution contract exists.
- Plugin `agents/**/*.md` files are loaded into the shared
  `AgentDefinitionCache` with namespaced `plugin_name:agent_name` names and can
  be used by `spawn_agent`, `explore_agent`, `plan_agent`, and team creation
  paths through the existing agent resolution contract.
- `.codex-plugin/plugin.json` is accepted as a compatible plugin metadata
  directory alongside `.claude-plugin/plugin.json`.
- RARA materializes the builtin `nowledge-mem` plugin under
  `~/.rara/builtin-plugins/nowledge-mem` and includes it as the lowest
  precedence discovery source before user, project, configured, and CLI plugin
  directories.
- The builtin `nowledge-mem` plugin registers streamable HTTP MCP, prompt-visible
  memory skills, and a namespaced subagent definition through the same runtime
  registries as external plugins.
- `rara plugin install <source>` accepts both local plugin directories and git
  sources. Git sources are cloned with `git clone --depth 1` into a temporary
  checkout before the existing plugin validation and workspace copy path runs.

## Open Risks

- Hook scripts may `require` Node modules that aren't installed in the user's
  environment. RARA cannot manage Node dependencies. Plugin `README.md` must
  document runtime requirements.
- Full matcher pattern evaluation is not complete yet. Current matcher
  evaluation matches the tool name only. Parenthesized input
  subpatterns such as `Bash(git status*)` are treated as tool-name matchers for
  `Bash`; input-level glob evaluation remains deferred.
- `prompt`-type hooks require prompt assembly integration that is not yet
  designed.
- No sandboxing of hook processes. A malicious plugin hook script has full user
  access. Trust is on the user who installed the plugin.

## Source Journals

- `docs/journal/2026-05-12-claude-plugin-runtime.md`
- `docs/journal/2026-05-12-main-sync-development-plan.md`
- `docs/journal/2026-07-20-plugin-runtime-bootstrap.md`
- `docs/journal/2026-07-19-plugin-source-discovery.md`
- `docs/journal/2026-07-20-plugin-dir-config.md`
- `docs/journal/2026-07-20-plugin-hook-matchers.md`
- `docs/journal/2026-07-20-plugin-runtime-bootstrap.md`
- `docs/journal/2026-07-21-plugin-non-tool-lifecycle-hooks.md`
- `docs/journal/2026-07-22-plugin-command-registry.md`
- `docs/journal/2026-07-25-extension-completion.md`
- `docs/journal/2026-07-30-nowledge-mem-builtin-plugin.md`
