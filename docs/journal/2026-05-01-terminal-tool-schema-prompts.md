# Terminal Tool Schema Prompts

## Context

RARA already had global command guidance in the instruction prompt, but the
terminal tool schemas still exposed only minimal descriptions. That left the
model without the same call-site guidance that Codex and Claude Code provide for
shell execution.

## Upstream Alignment

- Codex keeps shell workdir as an explicit tool parameter and resolves it
  against the turn cwd. It also routes patch application through a dedicated
  `apply_patch` path instead of encouraging shell-based patch execution.
- Claude Code puts Bash-specific discipline directly in the Bash tool
  description: keep the working directory stable, prefer dedicated tools, avoid
  shell-based file edits, avoid newline-separated command chaining, and keep
  commands sandboxed unless escalation is justified.

## Implementation Checkpoint

- Strengthened the `bash` tool description and input schema with command
  discipline for dedicated tools, `cwd`, sandbox escalation, background tasks,
  and shell-edit avoidance.
- Added `multi_edit` for ordered exact replacements within one file. It follows
  the Claude Code `MultiEdit` shape: the file must be read fully first, each
  `old_string` is applied in order against the current file state, and ambiguous
  sequential replacements are rejected.
- Strengthened `replace`, `replace_lines`, `multi_edit`, `apply_patch`, and
  `bash` descriptions so edit intent is visible in the tool schema itself:
  shell `sed`, `awk`, `perl`, heredocs, redirection, and ad-hoc scripts are not
  the preferred path when a direct edit tool can express the change.
- Normalized `rg`-based bash exploration labels so TUI progress shows semantic
  `Find files ...` / `Search ...` actions instead of raw shell command text, and
  so those actions are not duplicated as running commands.
- Added a Codex-style foreground Bash result contract: raw results now include
  `aggregated_output` and `duration_ms`, and model-facing compaction renders a
  stable exit-code, duration, and output block from the captured result.
- Strengthened background task tool descriptions so models know to inspect or
  stop long-running work instead of starting duplicates.
- Strengthened `pty_start` and PTY control descriptions so PTY is reserved for
  interactive terminal sessions while ordinary commands stay on `bash`.
- Added focused schema-description tests for Bash, background tasks, and PTY.

## Validation

- `cargo test bash_tool_schema_guides_command_discipline -- --nocapture`
- `cargo test background_task_tool_descriptions_point_to_run_in_background -- --nocapture`
- `cargo test pty_tool_schema_guides_interactive_command_discipline -- --nocapture`
- `cargo test streaming_call_reports_stdout_and_stderr_chunks -- --nocapture`
- `cargo test compacts_bash_results_with_exit_code_duration_and_aggregated_output -- --nocapture`
- `cargo test tools::file::tests::multi_edit -- --nocapture`
- `cargo test tools::file::tests::file_tool_descriptions_encode_safe_edit_contract -- --nocapture`
- `cargo test tools::patch::tests::apply_patch_description_encodes_safe_edit_contract -- --nocapture`
- `cargo test tui::render::tests::exploration_summary_uses_codex_style_search_labels -- --nocapture`
- `cargo test tui::render::tests::rg_bash_search_is_not_duplicated_as_running_tool -- --nocapture`
- `cargo test tui::runtime::events::tests::bash_rg_tool_use_is_shown_as_exploration -- --nocapture`
- `cargo check`
