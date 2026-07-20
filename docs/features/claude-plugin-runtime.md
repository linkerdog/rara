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
- Synchronous `PreToolUse` command-hook blocking in the agent tool execution
  path.
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
  "tool_name": "Bash",        // only for tool events
  "tool_input": { "command": "git status" },
  "tool_response": null,      // reserved for PostToolUse
  "last_assistant_message": null,
  "is_interrupt": null
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
reserved for a later structured output surface.

### Installation

```bash
rara plugin install https://github.com/clawmem-ai/clawmem-claude-code-plugin
rara plugin install ./local-plugin
rara plugin list
rara plugin remove clawmem
```

Installation copies/clones the plugin to `~/.rara/plugins/<name>/`. For git
URLs, a shallow clone is performed and kept for future updates.

### Skill Extension Summaries

Runtime bootstrap discovers `skills/<name>/SKILL.md` directories from loaded
plugins and appends compact summaries to the agent's available-skill listing.
Plugin skill names are exposed as `plugin_name:skill_name`, use
`scope: "plugin"`, and set `disable_model_invocation: true` until plugin skill
invocation is routed through the shared skill registry. This makes plugin skill
availability visible to the model and control surfaces without implying that
the existing `skill` tool can invoke those bodies yet.

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
| Plugin skills are prompt-visible summaries | `cargo test plugin_middleware::tests::registers_project_plugin_skill_summaries -- --nocapture` and `cargo test agent::tests::plugin_hooks::plugin_skill_summaries_are_prompt_visible_but_not_invokable_yet -- --nocapture` |
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
- Plugin `skills/<name>/SKILL.md` directories are exposed as prompt-visible
  summaries with namespaced `plugin_name:skill_name` names and plugin scope.
  They are marked `disable_model_invocation: true` because the `skill` tool
  still reads from the local `SkillManager`, not from plugin extension roots.

Next implementation slices:

1. Fix remaining lifecycle parity gaps before broad user-facing rollout:
   structured hook output observability and non-tool lifecycle dispatch beyond
   `SessionEnd`.
2. Add git-source install support on top of the existing local-directory
   `rara plugin install/list/remove` commands.
3. Feed plugin `.mcp.json`, commands, skills, and agents into the same
   structured extension-source registries used by native RARA features. Skills
   already have prompt-visible summaries; invocation and reload integration
   remain open.

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
