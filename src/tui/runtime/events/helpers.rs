pub(super) use crate::control_tokens::scrub_internal_control_tokens;
use crate::tool::ToolOutputStream;
use crate::tools::bash::BashCommandInput;
use crate::tui::state::TuiApp;
use crate::tui::terminal_event::{
    TerminalEvent, output_tail_preview as terminal_output_tail_preview,
};
use crate::tui::tool_text::{compact_delegate_rest, compact_instruction};

pub(super) fn is_oauth_prompt_message(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("open this url in a browser and enter the one-time code:")
        || lower.contains("open this url if the browser does not launch automatically:")
}

pub(super) fn is_exploration_tool_name(name: &str) -> bool {
    matches!(name, "list_files" | "read_file" | "glob" | "grep")
}

pub(super) fn exploration_action_label(message: &str) -> Option<String> {
    let mut parts = message.split_whitespace();
    let name = parts.next()?;
    let rest = parts.collect::<Vec<_>>().join(" ");
    match name {
        "list_files" => Some(format!(
            "List {}",
            if rest.is_empty() { "." } else { rest.as_str() }
        )),
        "read_file" => Some(format!(
            "Read {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        "glob" => Some(format!(
            "Glob {}",
            if rest.is_empty() {
                "workspace"
            } else {
                rest.as_str()
            }
        )),
        "grep" => Some(format!(
            "Search {}",
            if rest.is_empty() {
                "workspace"
            } else {
                rest.as_str()
            }
        )),
        "bash" => bash_rg_exploration_action_label(&rest),
        "explore_agent" => Some(if rest.is_empty() {
            "Delegate repository exploration".to_string()
        } else {
            format!("Delegate repository exploration: {rest}")
        }),
        _ => None,
    }
}

fn bash_rg_exploration_action_label(command: &str) -> Option<String> {
    let command = command.trim();
    if command.is_empty() || !contains_rg_invocation(command) {
        return None;
    }

    if command.split_whitespace().any(|part| part == "--files") {
        Some(format!("Find files {command}"))
    } else {
        Some(format!("Search {command}"))
    }
}

fn contains_rg_invocation(command: &str) -> bool {
    command
        .split([';', '|', '&'])
        .map(str::trim)
        .any(|segment| segment == "rg" || segment.starts_with("rg ") || segment.starts_with("rg\t"))
}

pub(super) fn planning_action_label(message: &str) -> Option<String> {
    let mut parts = message.split_whitespace();
    let name = parts.next()?;
    let rest = parts.collect::<Vec<_>>().join(" ");
    match name {
        "plan_agent" => Some(if rest.is_empty() {
            "Delegate plan refinement".to_string()
        } else {
            format!("Delegate plan refinement: {rest}")
        }),
        _ => None,
    }
}

pub(super) fn tool_action_label(message: &str) -> Option<String> {
    let mut parts = message.split_whitespace();
    let name = parts.next()?;
    if is_exploration_tool_name(name) {
        return None;
    }
    let rest = parts.collect::<Vec<_>>().join(" ");
    match name {
        "bash" => Some(format!(
            "Run {}",
            if rest.is_empty() {
                "command"
            } else {
                rest.as_str()
            }
        )),
        "apply_patch" => Some(format!(
            "Apply patch {}",
            if rest.is_empty() {
                "changes"
            } else {
                rest.as_str()
            }
        )),
        "write_file" => Some(format!(
            "Write {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        "replace" => Some(format!(
            "Edit {}",
            if rest.is_empty() {
                "file"
            } else {
                rest.as_str()
            }
        )),
        "replace_lines" => Some(format!(
            "Edit lines {}",
            if rest.is_empty() {
                "in file"
            } else {
                rest.as_str()
            }
        )),
        "spawn_agent" => Some(format!(
            "Delegate {}",
            compact_delegate_rest(&rest).unwrap_or_else(|| "sub-agent".to_string())
        )),
        "web_fetch" => Some(format!(
            "Fetch {}",
            if rest.is_empty() {
                "resource"
            } else {
                rest.as_str()
            }
        )),
        "web_search" => Some(format!(
            "Search {}",
            if rest.is_empty() {
                "web".to_string()
            } else {
                compact_instruction(&rest)
            }
        )),
        other => Some(format!(
            "Run {}",
            if rest.is_empty() { other } else { message }
        )),
    }
}

pub(super) fn exploration_note_lines(message: &str, planning_mode: bool) -> Vec<String> {
    let mut notes = Vec::new();
    for line in message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("/search ")
            || line.starts_with("/compact ")
            || line.starts_with("/plan ")
            || line.starts_with("/quit ")
            || line.starts_with("key=")
            || line.starts_with("history=")
            || line.starts_with("tokens=")
            || line.starts_with("ctx~=")
            || line.starts_with("waiting for model response")
        {
            continue;
        }
        if planning_mode && (is_planning_chatter(line) || looks_like_planning_summary(line)) {
            continue;
        }
        if !notes.iter().any(|existing| existing == line) {
            notes.push(line.to_string());
        }
    }
    notes
}

pub(super) fn is_planning_chatter(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("i will now ")
        || lower.starts_with("i will start by ")
        || lower.starts_with("i will use ")
        || lower.starts_with("i am ready to ")
        || lower.starts_with("i'm ready to ")
        || lower.starts_with("i need to ")
        || lower.starts_with("to continue ")
        || lower.starts_with("to fully ")
        || lower.starts_with("to understand ")
        || lower.starts_with("this is the final step")
        || lower.starts_with("based on the existing code")
}

pub(super) fn planning_note_lines(message: &str) -> Vec<String> {
    let mut notes = Vec::new();
    for line in message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if line.starts_with("/compact ")
            || line.starts_with("/plan ")
            || line.starts_with("/quit ")
            || line.starts_with("waiting for model response")
            || is_planning_chatter(line)
            || looks_like_planning_summary(line)
            || mentions_mutating_plan_action(line)
        {
            continue;
        }
        if !notes.iter().any(|existing| existing == line) {
            notes.push(line.to_string());
        }
    }
    notes
}

fn mentions_mutating_plan_action(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    lower.contains("apply_patch")
        || lower.contains("write_file")
        || lower.contains("apply the patch")
        || lower.contains("write the file")
        || lower.contains("edit ")
        || lower.contains("replace ")
        || lower.contains("modify ")
        || lower.contains("write ")
        || lower.contains("edit files")
        || lower.contains("modify the code")
        || lower.contains("implement the change")
}

pub(super) fn looks_like_planning_summary(line: &str) -> bool {
    let lower = line.trim().to_ascii_lowercase();
    lower.starts_with("i propose the following plan")
        || lower.starts_with("i propose the following")
        || lower.starts_with("based on the inspection")
        || lower.starts_with("based on the existing code")
        || lower.starts_with("the primary suggestions center on")
        || lower.starts_with("the core logic")
        || lower.starts_with("to provide comprehensive suggestions")
}

pub(super) fn format_tool_use(name: &str, input: &serde_json::Value) -> String {
    match name {
        "bash" => BashCommandInput::from_value(input.clone())
            .map(|request| format!("bash {}", request.summary()))
            .unwrap_or_else(|_| format!("{name} {input}")),
        "read_file" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("read_file {path}"))
            .unwrap_or_else(|| format!("{name} {input}")),
        "write_file" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("write_file {path}"))
            .unwrap_or_else(|| format!("{name} {input}")),
        "replace" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("replace {path}"))
            .unwrap_or_else(|| format!("{name} {input}")),
        "replace_lines" => format_replace_lines_use(input),
        "list_files" => input
            .get("path")
            .and_then(serde_json::Value::as_str)
            .map(|path| format!("list_files {path}"))
            .unwrap_or_else(|| format!("{name} {input}")),
        "grep" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<pattern>");
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(".");
            format!("grep {pattern} in {path}")
        }
        "glob" => {
            let pattern = input
                .get("pattern")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<pattern>");
            let path = input
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(".");
            format!("glob {pattern} in {path}")
        }
        "web_fetch" => input
            .get("url")
            .and_then(serde_json::Value::as_str)
            .map(|url| format!("web_fetch {url}"))
            .unwrap_or_else(|| format!("{name} {input}")),
        "web_search" => input
            .get("query")
            .and_then(serde_json::Value::as_str)
            .map(|query| format!("web_search {}", compact_instruction(query)))
            .unwrap_or_else(|| format!("{name} {input}")),
        "explore_agent" => format_instruction_tool_use("explore_agent", input),
        "plan_agent" => format_instruction_tool_use("plan_agent", input),
        "spawn_agent" => format_spawn_agent_use(input),
        "apply_patch" => format_apply_patch_use(input),
        "pty_start" => input
            .get("command")
            .and_then(serde_json::Value::as_str)
            .map(|command| format!("pty_start {}", compact_instruction(command)))
            .unwrap_or_else(|| format!("{name} {input}")),
        "pty_read" | "pty_status" | "pty_write" | "pty_kill" | "pty_stop" => {
            format_session_tool_use(name, input, "session_id")
        }
        "background_task_status" | "background_task_stop" => {
            format_session_tool_use(name, input, "task_id")
        }
        "background_task_list" | "pty_list" => name.to_string(),
        _ => format!("{name} {input}"),
    }
}

fn format_session_tool_use(name: &str, input: &serde_json::Value, id_key: &str) -> String {
    let id = input
        .get(id_key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty());
    if name == "pty_write" {
        let preview = input
            .get("input")
            .and_then(serde_json::Value::as_str)
            .map(summarize_terminal_input);
        return match (id, preview) {
            (Some(id), Some(preview)) => format!("{name} {id}: {preview}"),
            (Some(id), None) => format!("{name} {id}"),
            _ => format!("{name} {input}"),
        };
    }
    match id {
        Some(id) => format!("{name} {id}"),
        None if name == "pty_stop" || name == "background_task_stop" => name.to_string(),
        None => format!("{name} {input}"),
    }
}

fn summarize_terminal_input(input: &str) -> String {
    const MAX_CHARS: usize = 80;
    let normalized = input.replace('\n', "\\n").replace('\r', "\\r");
    if normalized.chars().count() <= MAX_CHARS {
        return normalized;
    }
    let mut truncated = normalized.chars().take(MAX_CHARS).collect::<String>();
    truncated.push('…');
    truncated
}

fn format_replace_lines_use(input: &serde_json::Value) -> String {
    let path = input
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let start_line = input
        .get("start_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let end_line = input
        .get("end_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    format!("replace_lines {path}:{start_line}-{end_line}")
}

fn format_instruction_tool_use(name: &str, input: &serde_json::Value) -> String {
    let instruction = input
        .get("instruction")
        .and_then(serde_json::Value::as_str)
        .map(compact_instruction)
        .unwrap_or_else(|| "instruction unavailable".to_string());
    format!("{name} {instruction}")
}

fn format_spawn_agent_use(input: &serde_json::Value) -> String {
    let agent_name = input
        .get("name")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("worker");
    let instruction = input
        .get("instruction")
        .and_then(serde_json::Value::as_str)
        .map(compact_instruction)
        .unwrap_or_else(|| "instruction unavailable".to_string());
    format!("spawn_agent {agent_name}: {instruction}")
}

pub(super) fn format_apply_patch_use(input: &serde_json::Value) -> String {
    let Some(patch) = input.get("patch").and_then(serde_json::Value::as_str) else {
        return "apply_patch".to_string();
    };

    let files = apply_patch_targets(patch);
    if files.is_empty() {
        "apply_patch".to_string()
    } else {
        format!("apply_patch {}", files.join(", "))
    }
}

fn apply_patch_targets(patch: &str) -> Vec<String> {
    let mut files = Vec::new();
    for line in patch.lines().map(str::trim) {
        let path = line
            .strip_prefix("*** Add File: ")
            .or_else(|| line.strip_prefix("*** Delete File: "))
            .or_else(|| line.strip_prefix("*** Update File: "))
            .or_else(|| line.strip_prefix("*** Move to: "));
        let Some(path) = path else {
            continue;
        };
        if !files.iter().any(|existing| existing == path) {
            files.push(path.to_string());
        }
    }
    files
}

pub(super) fn format_tool_result(name: &str, content: &str) -> String {
    if matches!(name, "explore_agent" | "plan_agent" | "spawn_agent") {
        if let Ok(value) = serde_json::from_str::<serde_json::Value>(content) {
            let summary = value
                .get("summary")
                .and_then(serde_json::Value::as_str)
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| first_non_empty_line(content));
            let mut rendered = if name == "spawn_agent" {
                let agent_name = value
                    .get("name")
                    .and_then(serde_json::Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .unwrap_or("worker");
                format!("{name} {agent_name}: {summary}")
            } else {
                format!("{name} {summary}")
            };
            if let Some(request) = value.get("request_user_input") {
                if let Some(question) = request.get("question").and_then(serde_json::Value::as_str)
                {
                    rendered.push_str(&format!("\nrequest_user_input: {}", question.trim()));
                }
                if let Some(options) = request.get("options").and_then(serde_json::Value::as_array)
                {
                    for option in options {
                        if let Some((label, description)) = option.as_array().and_then(|pair| {
                            Some((
                                pair.first()?.as_str()?,
                                pair.get(1)
                                    .and_then(serde_json::Value::as_str)
                                    .unwrap_or(""),
                            ))
                        }) {
                            rendered.push_str(&format!(
                                "\noption: {} | {}",
                                label.trim(),
                                description.trim()
                            ));
                        }
                    }
                }
                if let Some(note) = request.get("note").and_then(serde_json::Value::as_str) {
                    if !note.trim().is_empty() {
                        rendered.push_str(&format!("\nnote: {}", note.trim()));
                    }
                }
            }
            return rendered;
        }
        if content.trim_start().starts_with(&format!("{name} ")) {
            return content.trim().to_string();
        }
        return format!("{name} {}", first_non_empty_line(content));
    }
    if let Some(event) = TerminalEvent::from_tool_result(name, content, false) {
        return event.to_transcript_message();
    }
    if name == "bash"
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        let exit_code = value
            .get("exit_code")
            .and_then(serde_json::Value::as_i64)
            .unwrap_or(-1);
        let stdout = value
            .get("stdout")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let stderr = value
            .get("stderr")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default();
        let live_streamed = value
            .get("live_streamed")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false);
        let mut summary = if exit_code == 0 {
            "bash finished with exit code 0".to_string()
        } else if exit_code >= 0 {
            format!("bash failed with exit code {exit_code}")
        } else {
            "bash finished with unknown exit status".to_string()
        };
        if live_streamed {
            summary.push_str("\noutput streamed above");
        }
        append_bash_output_preview(&mut summary, stdout, stderr);
        return summary;
    }
    if name == "apply_patch"
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        return format_apply_patch_result(&value);
    }

    if name == "write_file"
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        return format_write_file_result(&value);
    }

    if name == "replace"
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        return format_replace_result(&value);
    }

    if name == "replace_lines"
        && let Ok(value) = serde_json::from_str::<serde_json::Value>(content)
    {
        return format_replace_lines_result(&value);
    }

    if name == "list_files" {
        return content.to_string();
    }

    if let Some(summary) = content
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        let summary = normalize_tool_result_summary(name, summary);
        let mut rendered = format!("{name}: {summary}");
        if content.contains("\nfull result:") {
            let remainder = content.lines().skip(1).collect::<Vec<_>>().join("\n");
            if !remainder.trim().is_empty() {
                rendered.push('\n');
                rendered.push_str(remainder.trim_end());
            }
        } else if content.contains("<persisted-output>") {
            rendered.push_str("\nfull result stored on disk");
        } else if content.contains("full_result_path=") {
            rendered.push_str("\nfull result stored on disk");
        } else if let Some(preview) = tool_result_preview(content) {
            rendered.push_str(&format!("\n{preview}"));
        }
        return rendered;
    }

    format!("{name}: {content}")
}

fn normalize_tool_result_summary(name: &str, summary: &str) -> String {
    let trimmed = summary.trim();
    let expected_prefix = format!("Tool {name} completed with ");
    if trimmed.starts_with(&expected_prefix) {
        return format!("{name} finished");
    }
    trimmed.to_string()
}

fn tool_result_preview(content: &str) -> Option<String> {
    let preview_lines = content
        .lines()
        .skip(1)
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .take(3)
        .map(ToString::to_string)
        .collect::<Vec<_>>();

    if preview_lines.is_empty() {
        None
    } else {
        Some(preview_lines.join("\n"))
    }
}

pub(super) fn format_tool_progress(name: &str, stream: ToolOutputStream, chunk: &str) -> String {
    let Some(visible_chunk) = output_tail_preview(chunk) else {
        return String::new();
    };
    let stream_label = match stream {
        ToolOutputStream::Stdout => "stdout",
        ToolOutputStream::Stderr => "stderr",
    };
    format!("{name} {stream_label}:\n{visible_chunk}\n")
}

pub(super) fn append_tool_progress(
    app: &mut TuiApp,
    name: &str,
    stream: ToolOutputStream,
    chunk: &str,
) -> bool {
    let rendered = format_tool_progress(name, stream, chunk);
    if rendered.is_empty() {
        return false;
    }

    if let Some(last) = app.active_turn.entries.last_mut()
        && last.role == "Tool Progress"
    {
        last.message.push_str(&rendered);
        limit_tool_progress_entry(&mut last.message);
        return true;
    }

    app.push_entry("Tool Progress", rendered);
    if let Some(last) = app.active_turn.entries.last_mut() {
        limit_tool_progress_entry(&mut last.message);
    }
    true
}

fn limit_tool_progress_entry(message: &mut String) {
    let line_count = message
        .as_bytes()
        .iter()
        .filter(|&&byte| byte == b'\n')
        .count();
    if line_count <= super::TOOL_PROGRESS_LINE_LIMIT {
        return;
    }

    let remove_lines = line_count - super::TOOL_PROGRESS_LINE_LIMIT;
    let mut removed = 0_usize;
    let mut cutoff = 0_usize;
    for (index, byte) in message.bytes().enumerate() {
        if byte == b'\n' {
            removed += 1;
            if removed == remove_lines {
                cutoff = index + 1;
                break;
            }
        }
    }

    let mut folded = String::from("... live output truncated ...\n");
    folded.push_str(&message[cutoff..]);
    *message = folded;
}

pub(super) fn format_apply_patch_result(value: &serde_json::Value) -> String {
    let status = value
        .get("status")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let files_changed = value
        .get("files_changed")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let added = value
        .get("line_delta")
        .and_then(|delta| delta.get("added"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let removed = value
        .get("line_delta")
        .and_then(|delta| delta.get("removed"))
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();

    let mut lines = vec![format!(
        "apply_patch {status} {files_changed} file(s) (+{added} -{removed})"
    )];

    append_path_group(&mut lines, "updated", value.get("updated_files"));
    append_path_group(&mut lines, "created", value.get("created_files"));
    append_path_group(&mut lines, "deleted", value.get("deleted_files"));

    if let Some(moves) = value
        .get("moved_files")
        .and_then(serde_json::Value::as_array)
    {
        let rendered = moves
            .iter()
            .filter_map(|entry| {
                let from = entry.get("from").and_then(serde_json::Value::as_str)?;
                let to = entry.get("to").and_then(serde_json::Value::as_str)?;
                Some(format!("{from} -> {to}"))
            })
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            lines.push(format!("moved: {}", rendered.join(", ")));
        }
    }

    if let Some(summary) = value.get("summary").and_then(serde_json::Value::as_array) {
        let preview = summary
            .iter()
            .filter_map(serde_json::Value::as_str)
            .take(4)
            .collect::<Vec<_>>();
        if !preview.is_empty() {
            let remaining = summary.len().saturating_sub(preview.len());
            lines.push("changes:".to_string());
            for line in preview {
                lines.push(format!("  {line}"));
            }
            if remaining > 0 {
                lines.push(format!("  ... {remaining} more change(s)"));
            }
        }
    }

    if let Some(diff_preview) = value
        .get("diff_preview")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        lines.push("diff:".to_string());
        lines.extend(diff_preview.lines().map(ToString::to_string));
    }

    lines.join("\n")
}

fn append_path_group(lines: &mut Vec<String>, label: &str, value: Option<&serde_json::Value>) {
    let Some(paths) = value.and_then(serde_json::Value::as_array) else {
        return;
    };
    let rendered = paths
        .iter()
        .filter_map(serde_json::Value::as_str)
        .take(4)
        .collect::<Vec<_>>();
    if rendered.is_empty() {
        return;
    }
    lines.push(format!("{label}: {}", rendered.join(", ")));
    let remaining = paths.len().saturating_sub(rendered.len());
    if remaining > 0 {
        lines.push(format!("  ... {remaining} more"));
    }
}

fn output_tail_preview(output: &str) -> Option<String> {
    terminal_output_tail_preview(output).map(|lines| lines.join("\n"))
}

fn append_bash_output_preview(summary: &mut String, stdout: &str, stderr: &str) {
    match (output_tail_preview(stdout), output_tail_preview(stderr)) {
        (Some(stdout_preview), Some(stderr_preview)) => {
            summary.push_str(&format!("\nstdout:\n{stdout_preview}"));
            summary.push_str(&format!("\nstderr:\n{stderr_preview}"));
        }
        (Some(stdout_preview), None) => {
            summary.push('\n');
            summary.push_str(&stdout_preview);
        }
        (None, Some(stderr_preview)) => {
            summary.push_str(&format!("\nstderr:\n{stderr_preview}"));
        }
        (None, None) => {}
    }
}

fn format_write_file_result(value: &serde_json::Value) -> String {
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let operation = value
        .get("operation")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("updated");
    let line_count = value
        .get("line_count")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let bytes_written = value
        .get("bytes_written")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();

    let mut lines = vec![format!(
        "write_file {operation} {path} ({line_count} lines, {bytes_written} bytes)"
    )];

    if let Some(previous_bytes) = value
        .get("previous_bytes")
        .and_then(serde_json::Value::as_u64)
    {
        let previous_lines = value
            .get("previous_line_count")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or_default();
        lines.push(format!(
            "previous: {previous_lines} lines, {previous_bytes} bytes"
        ));
    }

    lines.join("\n")
}

fn format_replace_result(value: &serde_json::Value) -> String {
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let replacements = value
        .get("replacements")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let line_delta = value
        .get("line_delta")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    let mut lines = vec![format!(
        "replace {path} {replacements} replacement(s) (Δlines={line_delta})"
    )];
    if let Some(old_preview) = value.get("old_preview").and_then(serde_json::Value::as_str) {
        lines.push(format!("old: {old_preview}"));
    }
    if let Some(new_preview) = value.get("new_preview").and_then(serde_json::Value::as_str) {
        lines.push(format!("new: {new_preview}"));
    }
    lines.join("\n")
}

fn format_replace_lines_result(value: &serde_json::Value) -> String {
    let path = value
        .get("path")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("<unknown>");
    let start_line = value
        .get("start_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let end_line = value
        .get("end_line")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let removed_lines = value
        .get("removed_lines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let inserted_lines = value
        .get("inserted_lines")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or_default();
    let line_delta = value
        .get("line_delta")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or_default();
    format!(
        "replace_lines {path}:{start_line}-{end_line}\nremoved={removed_lines} inserted={inserted_lines} line_delta={line_delta}"
    )
}

pub(super) fn exploration_result_note(message: &str) -> Option<String> {
    message.strip_prefix("explore_agent ").map(|summary| {
        let first_line = summary.lines().next().unwrap_or(summary).trim();
        format!("Sub-agent summary: {first_line}")
    })
}

pub(super) fn planning_result_note(message: &str) -> Option<String> {
    message.strip_prefix("plan_agent ").map(|summary| {
        let first_line = summary.lines().next().unwrap_or(summary).trim();
        format!("Sub-agent summary: {first_line}")
    })
}

pub(super) struct DelegatedRequestInput {
    pub(super) question: String,
    pub(super) options: Vec<(String, String)>,
    pub(super) note: Option<String>,
}

pub(super) fn subagent_request_input(message: &str) -> Option<DelegatedRequestInput> {
    if !(message.starts_with("explore_agent ")
        || message.starts_with("plan_agent ")
        || message.starts_with("spawn_agent "))
    {
        return None;
    }

    let mut question = None;
    let mut options = Vec::new();
    let mut note = None;
    for line in message
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
    {
        if let Some(value) = line.strip_prefix("request_user_input:") {
            question = Some(value.trim().to_string());
            continue;
        }
        if let Some(value) = line.strip_prefix("option:") {
            let value = value.trim();
            if let Some((label, description)) = value.split_once('|') {
                options.push((label.trim().to_string(), description.trim().to_string()));
            } else {
                options.push((value.to_string(), String::new()));
            }
            continue;
        }
        if let Some(value) = line.strip_prefix("note:") {
            let value = value.trim();
            if !value.is_empty() {
                note = Some(value.to_string());
            }
        }
    }

    Some(DelegatedRequestInput {
        question: question?,
        options,
        note,
    })
}

fn first_non_empty_line(text: &str) -> &str {
    text.lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or(text)
}

pub(super) fn format_error_chain(err: &anyhow::Error) -> String {
    use crate::redaction::redact_secrets;

    let mut lines = Vec::new();
    for (idx, cause) in err.chain().enumerate() {
        let rendered = redact_secrets(cause.to_string());
        if idx == 0 {
            lines.push(rendered);
        } else {
            lines.push(format!("caused by: {rendered}"));
        }
    }
    lines.join("\n")
}
