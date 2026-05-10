# RARA

RARA is a local-first coding agent CLI written in Rust. It provides a terminal
chat UI, multiple model providers, tool execution, workspace memory, and session
restore.

The project is still moving quickly. Treat the README as a technical snapshot of
the current runtime surface, not a stable product contract.

## Highlights

- 🖥️ Terminal-first coding agent UI with live tool progress and GFM markdown
  rendering.
- 🔌 Hosted, OpenAI-compatible, Ollama, Gemini, and local Candle model backends.
- 🛠️ File editing, shell, PTY, search, web, planning, memory, skill, and
  sub-agent tools.
- 🧠 Local workspace memory, project memory, thread restore, and thread fork.
- 🔐 Command sandboxing with macOS seatbelt and Linux bubblewrap support.
- 📦 Local skills from markdown, including `SKILL.md` directory skills.
- 🔍 `/context` and `/status` for inspecting runtime state.
- 🧵 Queued follow-up messages while the agent is busy or waiting for approval.
- 🎯 Sidechannel LLM classifier for auto-permission decisions and background
  task status.

## What It Does

- Runs an interactive terminal agent with a Ratatui-based TUI.
- Supports hosted providers and OpenAI-compatible endpoints.
- Runs local models through Candle-backed backends.
- Executes file, shell, search, web, planning, memory, skill, and sub-agent
  tools.
- Applies a sidechannel LLM classifier for auto-permission decisions and
  background task status, mirroring Claude Code's approach.
- Keeps workspace state local.
- Restores, lists, opens, and forks previous threads.
- Loads project and user instructions.
- Loads skills from local markdown files.
- Applies sandbox policy around command execution, with macOS seatbelt,
  Linux bubblewrap, or direct execution depending on the platform.

## Build

```bash
cargo build
```

Run the TUI directly from source:

```bash
cargo run -- tui
```

Install the local binary:

```bash
cargo install --path .
```

## CLI

```bash
rara tui
rara ask "summarize this repository"
rara login
rara login --device-auth
rara login --with-api-key
rara resume --last
rara resume <thread-id>
rara threads --limit 20
rara thread <thread-id>
rara fork <thread-id>
rara acp
```

Global provider overrides:

```bash
rara --provider deepseek --model deepseek-chat tui
rara --provider openai-compatible --base-url http://localhost:8080/v1 --model my-model tui
rara --provider ollama --model qwen3:latest tui
rara --provider local --model Qwen/Qwen3-0.6B tui
```

`RARA_API_KEY` can be used instead of passing `--api-key`.

## Providers

Current backend families:

- `codex`
- `deepseek`
- `kimi`
- `openrouter`
- `openai-compatible`
- `ollama` / `ollama-native`
- `ollama-openai`
- `gemini`
- `local` / `local-candle`
- `gemma4`
- `qwen3`
- `mock`

Provider state includes API key, base URL, model, reasoning settings, revision,
thinking flag, and context size where supported. OpenAI-compatible providers can
be managed as named endpoint profiles from the TUI model picker.

## TUI Commands

Inside the TUI:

- `/help` opens command help.
- `/model` changes provider, endpoint profile, API key, model, and reasoning
  settings.
- `/status` shows runtime status, context window budget, and compaction info.
- `/context` shows prompt sources, active context, memory selection, cache
  markers, and budget information.

Slash commands (`/model`, `/status`, `/help`) work even during agent rebuild.

The composer supports follow-up queuing while the agent is busy or waiting for
approval. Press Enter on an empty composer during a shell-approval prompt to
open a list picker (Once / Prefix / Always / Suggestion).

## Tools

Available tool families include:

- Shell: `bash`, `background_task_list`, `background_task_status`,
  `background_task_stop`.
- PTY: start/read/list/status/write/kill/stop.
- Files: `read_file`, `write_file`, `replace`, `replace_lines`,
  `apply_patch`, `list_files`.
- Search: `glob`, `grep`.
- Web: `web_fetch`, `web_search`.
- Planning: `enter_plan_mode`, `exit_plan_mode`.
- Memory: `remember_experience`, `retrieve_experience`,
  `retrieve_session_context`, `update_project_memory`.
- Skills: `skill`.
- Agents: `spawn_agent`, `explore_agent`, `plan_agent`, `team_create`.

Long tool output is folded in the transcript with a preview and a path to the
full result when needed.

## Sandbox

Command execution uses a platform-specific sandbox wrapper where available:

- macOS: non-PTY commands use a seatbelt profile through `sandbox-exec`; PTY
  commands currently run directly because `sandbox-exec` does not reliably
  preserve interactive PTY stdin.
- Linux: bubblewrap.
- Other or unsupported environments: direct execution.

Network access defaults to enabled for workspace-write command execution and can
be changed in config.

## Local State

RARA stores user config, credentials, workspace memory, thread state, tool
results, background task logs, and project memory locally.

Useful thread commands:

```bash
rara threads
rara thread <thread-id>
rara resume <thread-id>
rara resume --last
rara fork <thread-id>
```

## Skills

Skills are markdown files loaded from local search paths. RARA supports both:

- directory skills with `SKILL.md`;
- legacy `*.md` skill files.

Skills can define frontmatter metadata including name, title, and description.
Loaded skills are visible to the agent and can be invoked with the `skill` tool.

## Local Models

Local model support is backed by Candle. Build features expose optional
accelerators:

```bash
cargo build --features metal
cargo build --features cuda
cargo build --features accelerate
cargo build --features mkl
```

Model downloads use the local cache and provider/model configuration.

## Development

Common checks:

```bash
cargo fmt --check
cargo check
cargo test
```

Documentation policy:

- stable feature specs live in `docs/features/`;
- dated implementation checkpoints live in `docs/journal/`;
- active follow-up work lives in `docs/todo.md`.

## Star History

<a href="https://www.star-history.com/?repos=linkerdog%2Frara&type=date&legend=top-left">
 <picture>
   <source media="(prefers-color-scheme: dark)" srcset="https://api.star-history.com/chart?repos=linkerdog/rara&type=date&theme=dark&legend=top-left" />
   <source media="(prefers-color-scheme: light)" srcset="https://api.star-history.com/chart?repos=linkerdog/rara&type=date&legend=top-left" />
   <img alt="Star History Chart" src="https://api.star-history.com/chart?repos=linkerdog/rara&type=date&legend=top-left" />
 </picture>
</a>
