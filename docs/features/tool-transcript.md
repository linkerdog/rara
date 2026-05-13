# Tool Transcript

## Goal

Keep tool execution visible in the TUI without collapsing long-running work into a single post-hoc summary.

The transcript should move toward Codex/Claude-style tool visibility:

- tool uses should identify what they touched;
- edit tools should summarize file-level changes;
- shell execution should surface live stdout/stderr updates while the command is still running;
- queued follow-up messages should distinguish between:
  - messages waiting for the next tool/result boundary;
  - messages already queued for end-of-turn submission.

## Current Contract

### Edit tools

- `apply_patch` tool-use rows must include the touched file paths when they can be derived from the patch.
- `apply_patch` tool-result rows must summarize:
  - files changed;
  - line delta;
  - created / updated / deleted / moved files;
  - a short change preview.
- `write_file` and `replace` must render file-aware summaries instead of generic action labels.
- `write_file` follows the Claude-style Write/Edit split: use it for new files
  or intentional full-file rewrites, and prefer diff-shaped edit tools for
  modifications to existing files. Claude's own tool guidance says Write
  overwrites existing files, existing files should be read first in read-gated
  environments, and Edit is preferred for modifying existing files because it
  sends only the diff.
- Shell file-writing fallbacks should be treated as a different and riskier
  path. Claude's Bash/PowerShell guidance routes file writes through Write
  rather than `echo >`, `cat <<EOF`, `Set-Content`, or `Out-File`, and routes
  file edits through Edit rather than `sed` or `awk`. RARA should therefore not
  let a failed or oversized `write_file` call silently degrade into PTY,
  heredoc, redirection, `sed`, `perl`, or script-based file edits when direct
  edit tools or `apply_patch` can express the change.
- Codex uses `apply_patch` as the primary reviewable edit surface. Its heredoc
  support is an `apply_patch` transport shape, not a general recommendation to
  overwrite arbitrary files through shell heredocs.
- For large generated or rewritten files, the model-facing rule is to keep the
  write scoped to the target file, inspect the actual tool result, and verify
  with a direct read, diff, or focused check when persistence is uncertain.
  Large-write failure is a tool/result problem to diagnose, not permission to
  batch multiple shell writes into one opaque command.
- `replace` is an exact-match edit tool. When file read-state tracking is
  enabled, it still requires the file to have been read at least once, but it may
  proceed after a partial read because the edit re-reads the current file and
  requires `old_string` to match exactly once. Line-number-only edits such as
  `replace_lines` must continue to require a full read first.

### Shell execution

- `bash` tool execution must emit live transcript updates while stdout/stderr are still being produced.
- `bash` tool descriptions and input schema must carry the command discipline
  that the model sees at call time:
  - prefer dedicated RARA tools for file search, reads, and edits;
  - use `cwd` instead of prepending `cd`;
  - avoid newline-separated shell chaining;
  - issue independent validation commands as separate tool calls instead of
    combining them with `&&`, `;`, or pipelines only to run them together;
  - avoid adding `2>&1`, `head`, `tail`, or `grep` only to reduce displayed
    output, because the tool/result layer preserves stdout/stderr and provides a
    bounded model-facing preview;
  - run only non-interactive commands through `bash`; avoid editors, pagers,
    REPLs, prompts, and TUI programs;
  - use `git commit -m` or `git commit -F` for commits, never bare `git commit`
    that waits for an editor;
  - keep commands sandboxed unless escalation is justified by user request or
    clear sandbox failure evidence;
  - use background task controls for long-running non-interactive commands.
- The `bash` tool may normalize simple absolute working-directory prefixes
  before execution. A command shaped like `cd /absolute/path && <command>` can
  be converted into `cwd=/absolute/path` plus `<command>` when no explicit
  `cwd` or `program` field is present. This normalization must stay
  conservative: do not rewrite relative paths, empty command tails, or complex
  shell syntax where `cd` may be part of intentional shell state.
- The final `bash` transcript row should keep the exit code and avoid
  duplicating large output that was already streamed live; when live streaming
  was shown, the rendered row should use a compact summary or truncated preview
  rather than reprinting the full output block.
- The final foreground `bash` tool-result payload must still expose `stdout`,
  `stderr`, `aggregated_output`, `model_preview_output`, `exit_code`, and
  `duration_ms`. `aggregated_output` remains the raw combined capture.
  `model_preview_output` is the model-facing head/tail preview, with failed
  commands biased toward the tail so error diagnostics remain visible without
  requiring shell-side `2>&1` redirection.
- Oversized tool results should be persisted to disk and replaced in model
  context with a bounded preview plus the display-oriented continuation lines
  described below. The full JSON payload remains inspectable from that path.
- A single tool-result batch should enforce an aggregate model-facing budget so
  parallel tool calls cannot combine many individually acceptable results into
  one oversized follow-up turn. The final compacted batch must fit the aggregate
  budget, not only shorten the first oversized item encountered.
- Compact shell results must be composable at the source. The compact `bash`
  status line should describe only the process outcome, such as `finished with
  exit code 0` or `failed with exit code 101`; renderer layers may prepend the
  tool name when needed. Source compactors should not emit `bash finished`
  because that forces downstream renderers either to duplicate the tool name or
  to carry bash-specific string cleanup.
- Persisted oversized tool results should expose a display-oriented continuation
  line:
  - `[tool_result truncated]`
  - `full result: <path>`

  Legacy wrappers or key/value markers such as `<persisted-output>` or
  `full_result_path=<path>` may remain readable for backward compatibility, but
  new model-facing compact results should use the display contract above. This
  keeps final transcript rows human-readable while preserving a stable path for
  full output inspection.
- When shell execution pauses on a human approval request, the approval card should take visual priority over older live stdout/stderr progress from the same turn.
- Approval choices should describe both the action and its scope, such as:
  - allow only the current command;
  - allow commands with the matching prefix for the current session;
  - allow shell commands for the current session;
  - deny the command.
- OpenAI-compatible chat endpoints must keep approved shell command results as
  protocol-level tool messages before the runtime continuation message. DeepSeek
  v4/pro history folding for missing reasoning metadata is only valid when
  DeepSeek thinking mode is explicitly enabled; the default DeepSeek request body
  must preserve assistant tool calls and adjacent tool results so the model can
  continue after approval.
- If one assistant turn contains visible text followed by tool calls, render the
  visible text first and then execute the tool calls. Visible text is progress
  output, not an end-of-turn signal, while structured tool calls are still
  pending.
- DeepSeek V4 DSML is a fallback parser path, not the primary official API
  contract. Official DeepSeek API responses should prefer protocol-level
  `tool_calls`; if a compatible endpoint leaks DSML text, RARA should parse it
  with one shared DeepSeek V4 DSML parser and scrub the same blocks from the TUI.
- The default TUI terminal mode should preserve native terminal text selection.
  Mouse capture may be added later only behind an explicit opt-in, because
  terminal mouse reporting steals drag and wheel events from the host terminal.
- Edit tool results should expose a diff-like preview in the transcript instead
  of rendering only `old=` and `new=` summary lines.
- background bash tasks must be inspectable without imposing a fixed task-count limit:
  - `background_task_list` lists known background tasks;
  - `background_task_status` reads status and recent output for one task;
  - `background_task_stop` stops one task, or all running background bash tasks
    when no task id is supplied.

### PTY sessions

- `pty_start` tool descriptions and input schema must frame PTY as an
  interactive-command surface. Ordinary non-interactive commands should use
  `bash`, while PTY sessions should preserve the same `cwd` guidance as shell
  execution. Runtime sandboxing is platform-dependent: with the macOS seatbelt
  backend, PTY commands currently run unsandboxed because `sandbox-exec` does
  not preserve interactive PTY stdin reliably.
- PTY sessions must be inspectable and stoppable without imposing a fixed
  session-count limit:
  - `pty_list` lists known PTY sessions;
  - `pty_status` reads status and recent output for one session;
  - `pty_stop` stops one session, or all running PTY sessions when no session id
    is supplied.
- `pty_kill` and `pty_stop` use a two-step stop state for running sessions:
  return `killing` after the stop request is sent, then report `killed` only
  after the PTY reader observes EOF. Completed sessions stay `completed` when a
  later stop request is submitted.
- `pty_read`, `pty_write`, and `pty_kill` remain supported for direct session
  interaction and backward compatibility.

### Queued follow-up messages

- While a turn is running, follow-up user messages are not dropped.
- If a follow-up is entered during a query turn, it first waits for the next tool/result boundary.
- Once that boundary is crossed, the message is promoted into the ordinary end-of-turn queue.
- If the turn finishes before another boundary appears, the pending follow-up is promoted at turn completion.
- If a shell approval, plan approval, or other pending interaction is active,
  plain text submitted from the composer is queued as a follow-up instead of
  starting a new model turn. Request-input prompts remain answer paths, and
  numeric shortcuts remain option-selection paths.
- Queued follow-up visibility belongs in the active transcript, not in the
  composer body. The composer remains the input surface and may show only a
  short hint.
- The active transcript should render queued follow-up state as a dedicated
  status cell after any pending interaction card. This preserves option
  visibility for approval cards and keeps queued user input in chronological
  turn context.
- The queued status cell should render the two queues separately:
  - `Messages to be submitted after next tool call`
  - `Queued follow-up messages`

### TUI modular rendering boundary

The TUI should keep input state, display data, and visual cells separate:

- State modules own queue and interaction state. They should not encode Ratatui
  lines, spans, colors, or layout decisions.
- Small display-data builders, such as the queued follow-up section builder,
  may translate state into renderer-neutral structs. These builders should be
  deterministic and cheap to test without a terminal frame.
- History cells own transcript rendering. A cell should render one semantic
  thing: pending interaction, queued follow-up, running command, completed
  command, thinking group, or message text.
- The bottom pane owns composer text, cursor placement, and one-line hints. It
  should not render durable transcript status, approval options, queued
  follow-up bodies, or long-lived query progress text such as `Working` /
  `Sending prompt to model`. Runtime progress belongs in transcript/status
  events where it can scroll with the turn and remain ordered with tool output.
- Active-turn composition owns ordering. Pending interactions must be placed
  before queued follow-up status so approval options remain visible. Queued
  status should be appended near the newest active cell instead of being
  rewritten into an older transcript location.
- Tool-result compactors should emit renderer-neutral text. Renderer helpers
  may add the tool name, styling, and truncation frame, but should not need
  bash-specific source cleanup for normal output.
- All text that can reach the terminal renderer must pass through one
  display-sanitization boundary before it is printed or converted into markdown
  display lines. This includes streaming agent deltas, committed agent
  messages, live progress events such as Thinking/Exploring/Running, terminal
  event previews, and inline history insertion. The sanitizer must preserve
  user-visible text and line boundaries while removing terminal control effects
  such as ANSI/OSC escape sequences, carriage returns, backspaces, bells, and
  other non-printing controls. Tabs should be normalized to spaces so display
  width and wrapping stay stable.
- The transcript data model may still preserve richer raw payloads where needed,
  but the TUI must never `Print` raw model/tool text that can move the terminal
  cursor or rewrite previous cells. Rendering tests should cover both active
  streaming text and committed transcript text when this contract changes.
- Code-level ownership lives in a single display-sanitization module. Renderer
  code should call that module instead of duplicating ANSI/control-character
  stripping. Inline history insertion must sanitize full `Line` objects before
  both width/row calculation and terminal printing, so row accounting and
  visible output are derived from the same display-safe text.

This mirrors the Codex-style split where queued follow-up input is active
transcript status and the composer remains a prompt/input surface. It also keeps
future cells composable: adding a new pending interaction or queue type should
usually require a new display-data section or cell, not another special case in
the bottom pane.

### Running query cancellation

- When no overlay is open and a model query is running, `Esc` requests
  cancellation for the current query.
- Cancellation is cooperative: provider streaming loops should check the query
  cancellation token and return a local cancellation error instead of leaving the
  TUI stuck in `streaming model output`.
- Foreground tool calls must receive the same cancellation context. For `bash`,
  cancellation should stop the child process group, emit a cancellation
  diagnostic, and return a local `cancelled by user` error instead of waiting
  forever on stdout, stderr, or the process exit.
- Cancellation must preserve the current agent state so the user can continue
  from the same thread after the task exits.

## Non-Goals

- This does not yet implement the full Codex "interrupt and send immediately" steer path.
- This does not yet provide a fully separate command-pane widget for bash output; the current contract only guarantees live transcript visibility.
