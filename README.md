# RARA

**A fast, local-first coding agent for your terminal.**

RARA runs directly on your machine — no cloud dependency required. Pick your
model, ask it to write code or explore your repo, and watch it work through a
rich TUI with live streaming output, syntax highlighting, and side-by-side
context.

---

## Why RARA

- **Runs locally.** Your code, your models, your disk. Nothing leaves your
  machine unless you choose a hosted provider.
- **Model freedom.** Use Claude, DeepSeek, Gemini, Ollama, any
  OpenAI-compatible endpoint, or local Candle models — switch providers and
  models mid-session with `/model`.
- **Full tool suite.** Edit files, run shell commands, search code, browse the
  web, spawn sub-agents, update project memory, and load local skills.
- **Sandbox by default.** On macOS and Linux, command execution is wrapped in a
  platform sandbox; you control network and filesystem access per command.
- **Session persistence.** Every thread is saved, restorable, forkable, and
  searchable — pick up where you left off.

---

## Quick Start

```bash
git clone https://github.com/linkerdog/rara.git
cd rara
cargo run -- tui
```

Choose a provider on first launch — OpenAI, DeepSeek, Ollama, or a local
Candle model with zero setup.

For `RARA_API_KEY`, set the environment variable or pass `--api-key`.

```bash
# Start a TUI session with a specific provider
rara --provider deepseek --model deepseek-chat tui

# Ask a one-shot question
rara ask "summarize this repo"

# Resume your last session
rara resume --last
```

---

## TUI

The terminal UI gives you a full development environment:

- **Streaming responses** — watch the agent think and act in real time.
- **Live bash output** — stdout/stderr appear as the command runs.
- **Syntax highlighting** — code blocks, diffs, and inline code are colorized.
- **Sidebar context** — current-turn token usage, model info, open files, and
  child sessions at a glance.
- **Slash commands** — `/model` to switch providers, `/status` for runtime
  state, `/context` for prompt diagnostics, `/help` for available commands.
  All slash commands work during agent rebuild.
- **Approval card** — when a shell command needs approval, use Up/Down plus
  Enter, or press `1`-`4`, directly on the transcript card.
- **Follow-up queuing** — type ahead while the agent is busy; your messages
  queue and process in order.

---

## Providers

| Family | Description |
|--------|-------------|
| `codex` | OpenAI Codex (GPT-based coding agent) |
| `deepseek` | DeepSeek via hosted API |
| `gemini` | Google Gemini via hosted API |
| `kimi` | Moonshot Kimi via hosted API |
| `openrouter` | OpenRouter gateway |
| `openai-compatible` | Any OpenAI-compatible endpoint (custom base URL) |
| `ollama` / `ollama-openai` | Local Ollama inference |
| `local` / `local-candle` | Hugging Face Candle (local, no server needed) |
| `mock` | Deterministic test backend |

Provider state includes API key, base URL, model name, reasoning effort,
and context window size. OpenAI-compatible providers can be saved as named
profiles for quick switching.

---

## Tools

RARA gives the agent access to your workspace:

| Category | Tools |
|----------|-------|
| **Shell** | `bash`, background task list / status / stop |
| **PTY** | start, read, write, list, status, kill, stop |
| **Files** | read, write, edit (`replace`, `replace_lines`, `apply_patch`), list |
| **Search** | `glob`, `grep` |
| **Web** | `web_fetch`, `web_search` |
| **Planning** | `enter_plan_mode`, `exit_plan_mode` |
| **Memory** | `remember_experience`, `retrieve_experience`, `retrieve_session_context`, `update_project_memory` |
| **Skills** | `skill` (local `SKILL.md` files) |
| **Agents** | `spawn_agent`, `explore_agent`, `plan_agent`, `team_create` |

A **sidechannel LLM classifier** (using a lightweight auxiliary model)
evaluates shell and web tool calls before execution — blocking dangerous
commands and auto-approving safe ones, following the same pattern as
Claude Code's YOLO classifier.

Long tool output is folded in the transcript with a preview and a path to the
full result.

---

## Sandbox

Command execution is sandboxed by default:

| Platform | Method |
|----------|--------|
| macOS | `sandbox-exec` with seatbelt profile |
| Linux | `bubblewrap` |
| Other | Direct execution (no sandbox) |

PTY sessions on macOS currently run outside the seatbelt sandbox due to
PTY/seatbelt interaction constraints.

---

## Where State Lives

- `~/.config/rara/` — config, profiles, auth, access logs
- `~/.cache/rara/` — model downloads, runtime metadata
- `~/.local/share/rara/` — workspace thread logs and session data
- `<workspace>/.rara/` — per-project memory and lock files

---

## License

Apache 2.0
