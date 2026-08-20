use serde_json::{Value, json};

pub(super) const INLINE_CHAR_BUDGET: usize = 8_000;
const FILE_LIST_PREVIEW_LIMIT: usize = 200;
const MATCH_PREVIEW_LIMIT: usize = 40;
const LARGE_PREVIEW_HEAD: usize = 4_000;
const LARGE_PREVIEW_TAIL: usize = 4_000;
const BASH_SUCCESS_HEAD_CHARS: usize = 2_000;
const BASH_SUCCESS_TAIL_CHARS: usize = 2_000;
const BASH_ERROR_HEAD_CHARS: usize = 1_000;
const BASH_ERROR_TAIL_CHARS: usize = 3_000;
pub(super) fn compact_list_files(input: &Value, result: &Value) -> String {
    let files = result
        .get("files")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let rendered = files
        .iter()
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let summary = summarize_tool_result("list_files", input, result);
    format!("{summary}\nPreview:\n{rendered}")
}

pub(super) fn compact_read_file(input: &Value, result: &Value) -> String {
    let content = result
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let total_chars = content.chars().count();
    let preview = truncate_text(content, INLINE_CHAR_BUDGET.min(LARGE_PREVIEW_HEAD));
    let summary = summarize_tool_result("read_file", input, result);
    let metadata = read_file_metadata_line(result);
    let body = if preview.chars().count() < total_chars {
        format!("Content preview:\n{preview}\n... truncated.")
    } else {
        format!("Content:\n{preview}")
    };
    format!("{summary}\n{metadata}\n{body}")
}

fn read_file_metadata_line(result: &Value) -> String {
    let start = result
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or(1);
    let end = result.get("end_line").and_then(Value::as_u64).unwrap_or(0);
    let next = result
        .get("next_offset")
        .and_then(Value::as_u64)
        .map(|n| format!("{n}"))
        .unwrap_or_else(|| "none".to_string());
    let total = if result
        .get("total_lines_exact")
        .and_then(Value::as_bool)
        .unwrap_or(true)
    {
        format!(
            "{}",
            result
                .get("total_lines")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )
    } else {
        result
            .get("observed_lines")
            .and_then(Value::as_u64)
            .map(|n| format!("{n}+"))
            .unwrap_or_else(|| "?".to_string())
    };
    format!(
        "[file size] start_line={start}, end_line={end}, next_offset={next}, total_lines={total}"
    )
}

pub(super) fn compact_glob(result: &Value) -> String {
    let matches = result
        .get("matches")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = matches.len();
    let preview = matches
        .iter()
        .take(FILE_LIST_PREVIEW_LIMIT)
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let remaining = total.saturating_sub(FILE_LIST_PREVIEW_LIMIT);
    let summary = summarize_tool_result("glob", &Value::Null, result);
    if remaining > 0 {
        format!("{summary}\nPreview:\n{preview}\n... {remaining} more omitted.")
    } else {
        format!("{summary}\nPreview:\n{preview}")
    }
}

pub(super) fn compact_grep(result: &Value) -> String {
    let matches = result
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let total = matches.len();
    let preview = matches
        .iter()
        .take(MATCH_PREVIEW_LIMIT)
        .map(|entry| {
            let file = entry
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let line = entry.get("line").and_then(Value::as_u64).unwrap_or(0);
            let content = entry
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let ctx_hint = entry
                .get("context")
                .and_then(Value::as_array)
                .filter(|a| !a.is_empty())
                .map(|a| format!(" (+{} context lines)", a.len()));
            format!(
                "{file}:{line}: {content}{}",
                ctx_hint.as_deref().unwrap_or("")
            )
        })
        .collect::<Vec<_>>()
        .join("\n");
    let remaining = total.saturating_sub(MATCH_PREVIEW_LIMIT);
    let summary = summarize_tool_result("grep", &Value::Null, result);
    if remaining > 0 {
        format!("{summary}\nPreview:\n{preview}\n... {remaining} more omitted.")
    } else {
        format!("{summary}\nPreview:\n{preview}")
    }
}

pub(super) fn compact_web_fetch(input: &Value, result: &Value) -> String {
    let content = result
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let total_chars = content.chars().count();
    let preview = truncate_text(content, LARGE_PREVIEW_HEAD);
    let summary = summarize_tool_result("web_fetch", input, result);
    if preview.chars().count() < total_chars {
        format!("{summary}\nContent preview:\n{preview}\n... truncated.")
    } else {
        format!("{summary}\nContent:\n{preview}")
    }
}

pub(super) fn compact_web_search(input: &Value, result: &Value) -> String {
    let content = result
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let total_chars = content.chars().count();
    let preview = truncate_text(content, LARGE_PREVIEW_HEAD);
    let summary = summarize_tool_result("web_search", input, result);
    if preview.chars().count() < total_chars {
        format!("{summary}\nResults preview:\n{preview}\n... truncated.")
    } else {
        format!("{summary}\nResults:\n{preview}")
    }
}

pub(super) fn compact_apply_patch(result: &Value) -> String {
    let status = result
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    let files_changed = result
        .get("files_changed")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let hunks_applied = result
        .get("hunks_applied")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let summary_items = result
        .get("summary")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let preview = summary_items
        .iter()
        .take(10)
        .filter_map(Value::as_str)
        .collect::<Vec<_>>()
        .join("\n");
    let remainder = summary_items.len().saturating_sub(10);
    if remainder > 0 {
        format!(
            "Patch {status}: {files_changed} file(s), {hunks_applied} hunk(s).\nChanges:\n{preview}\n... {remainder} more change(s) omitted."
        )
    } else {
        format!(
            "Patch {status}: {files_changed} file(s), {hunks_applied} hunk(s).\nChanges:\n{preview}"
        )
    }
}

pub(super) fn compact_generic(summary: &str, result: &Value) -> String {
    let rendered = serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string());
    format!(
        "{summary}\nPayload:\n{}",
        truncate_text(&rendered, LARGE_PREVIEW_HEAD)
    )
}

pub(super) fn render_persisted_compact_result(
    inline: &str,
    stored_path: &std::path::Path,
) -> String {
    format!(
        "{}\n\n[tool_result truncated]\nfull result: {}",
        head_tail_text(inline, LARGE_PREVIEW_HEAD, LARGE_PREVIEW_TAIL),
        stored_path.display()
    )
}

pub(super) fn compact_bash(result: &Value) -> String {
    if let Some(task_id) = result.get("background_task_id").and_then(Value::as_str) {
        let status = result
            .get("status")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        let output_path = result
            .get("output_path")
            .and_then(Value::as_str)
            .unwrap_or("<unknown>");
        let exit_code = result
            .get("exit_code")
            .and_then(Value::as_i64)
            .map(|code| code.to_string())
            .unwrap_or_else(|| "pending".to_string());
        return format!(
            "bash started in background.\nTask id: {task_id}\nStatus: {status}\nExit code: {exit_code}\nOutput path: {output_path}\nUse background_task_status with this task id to inspect output."
        );
    }

    let duration_ms = result.get("duration_ms").and_then(Value::as_u64);
    let output = result
        .get("model_preview_output")
        .and_then(Value::as_str)
        .filter(|output| !output.is_empty())
        .map(str::to_string)
        .or_else(|| {
            result
                .get("aggregated_output")
                .and_then(Value::as_str)
                .filter(|output| !output.is_empty())
                .map(|output| {
                    let exit_code = result.get("exit_code").and_then(Value::as_i64);
                    model_preview_bash_output(output, exit_code)
                })
        })
        .unwrap_or_else(|| {
            let stdout = result
                .get("stdout")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let stderr = result
                .get("stderr")
                .and_then(Value::as_str)
                .unwrap_or_default();
            match (stdout.is_empty(), stderr.is_empty()) {
                (true, true) => String::new(),
                (false, true) => stdout.to_string(),
                (true, false) => prefix_stderr_lines(stderr),
                (false, false) => {
                    let separator = if stdout.ends_with('\n') { "" } else { "\n" };
                    format!("{stdout}{separator}{}", prefix_stderr_lines(stderr))
                }
            }
        });
    let mut rendered = render_bash_outcome_summary(result);
    if let Some(duration_ms) = duration_ms {
        rendered.push_str(&format!("\nDuration: {duration_ms} ms"));
    }
    rendered.push_str("\nOutput:\n");
    rendered.push_str(&output);
    rendered
}

pub(crate) fn render_bash_outcome_summary(result: &Value) -> String {
    let legacy_exit_code = result.get("exit_code").and_then(Value::as_i64);
    let mut rendered = render_process_outcome(result, legacy_exit_code);
    if let Some(failure) = result.get("sandbox_failure") {
        let backend = failure
            .get("backend")
            .and_then(Value::as_str)
            .unwrap_or("unknown");
        match failure.get("kind").and_then(Value::as_str) {
            Some("policy_denied") => {
                rendered.push_str(&format!("\nSandbox: policy denied ({backend})"));
            }
            Some("sandboxed_process_signaled") => {
                rendered.push_str(&format!("\nSandbox: process signaled ({backend})"));
            }
            _ => {}
        }
    }
    rendered
}

fn render_process_outcome(result: &Value, legacy_exit_code: Option<i64>) -> String {
    let termination = result.get("termination");
    match termination
        .and_then(|value| value.get("kind"))
        .and_then(Value::as_str)
    {
        Some("exit") => {
            let code = termination
                .and_then(|value| value.get("code"))
                .and_then(Value::as_i64)
                .or(legacy_exit_code);
            match code {
                Some(0) => "finished with exit code 0".to_string(),
                Some(code) => format!("failed with exit code {code}"),
                None => "finished with unknown exit status".to_string(),
            }
        }
        Some("signal") => {
            let signal = termination
                .and_then(|value| value.get("signal"))
                .and_then(Value::as_i64);
            let name = termination
                .and_then(|value| value.get("name"))
                .and_then(Value::as_str);
            match (name, signal) {
                (Some(name), Some(signal)) => {
                    format!("terminated by {name} (signal {signal})")
                }
                (None, Some(signal)) => format!("terminated by signal {signal}"),
                _ => "terminated by an unknown signal".to_string(),
            }
        }
        _ => match legacy_exit_code {
            Some(0) => "finished with exit code 0".to_string(),
            Some(code) => format!("failed with exit code {code}"),
            None => "finished with unknown exit status".to_string(),
        },
    }
}

fn prefix_stderr_lines(stderr: &str) -> String {
    stderr
        .split_inclusive('\n')
        .map(|line| format!("[stderr] {line}"))
        .collect()
}

pub(super) fn compact_write_file(result: &Value) -> String {
    let path = result
        .get("path")
        .and_then(Value::as_str)
        .unwrap_or("<unknown>");
    let operation = result
        .get("operation")
        .and_then(Value::as_str)
        .unwrap_or("updated");
    let line_count = result
        .get("line_count")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let bytes_written = result
        .get("bytes_written")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let mut rendered =
        format!("write_file {operation} {path}\nlines={line_count} bytes={bytes_written}");
    if let Some(previous_bytes) = result.get("previous_bytes").and_then(Value::as_u64) {
        let previous_lines = result
            .get("previous_line_count")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        rendered.push_str(&format!(
            "\nprevious_lines={previous_lines} previous_bytes={previous_bytes}"
        ));
    }
    rendered
}

pub(super) fn compact_replace(input: &Value, result: &Value) -> String {
    let path = result
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| input.get("path").and_then(Value::as_str))
        .unwrap_or("<unknown>");
    let replacements = result
        .get("replacements")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let line_delta = result
        .get("line_delta")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let old_string = input
        .get("old_string")
        .and_then(Value::as_str)
        .or_else(|| result.get("old_preview").and_then(Value::as_str))
        .unwrap_or_default();
    let new_string = input
        .get("new_string")
        .and_then(Value::as_str)
        .or_else(|| result.get("new_preview").and_then(Value::as_str))
        .unwrap_or_default();
    let diff = simple_patch_diff(path, old_string, new_string);
    format!("replace {path}\nreplacements={replacements} line_delta={line_delta}\ndiff:\n{diff}")
}

pub(super) fn compact_replace_lines(input: &Value, result: &Value) -> String {
    let path = result
        .get("path")
        .and_then(Value::as_str)
        .or_else(|| input.get("path").and_then(Value::as_str))
        .unwrap_or("<unknown>");
    let start_line = result
        .get("start_line")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let end_line = result
        .get("end_line")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let removed_lines = result
        .get("removed_lines")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let inserted_lines = result
        .get("inserted_lines")
        .and_then(Value::as_u64)
        .unwrap_or_default();
    let line_delta = result
        .get("line_delta")
        .and_then(Value::as_i64)
        .unwrap_or_default();
    let old_string = result
        .get("removed_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_string = input
        .get("new_string")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let diff = simple_patch_diff(path, old_string, new_string);
    format!(
        "replace_lines {path}:{start_line}-{end_line}\nremoved={removed_lines} inserted={inserted_lines} line_delta={line_delta}\ndiff:\n{diff}"
    )
}

fn simple_patch_diff(path: &str, old_string: &str, new_string: &str) -> String {
    let mut lines = vec![
        "*** Begin Patch".to_string(),
        format!("*** Update File: {path}"),
        "@@".to_string(),
    ];
    lines.extend(old_string.lines().map(|line| format!("-{line}")));
    lines.extend(new_string.lines().map(|line| format!("+{line}")));
    lines.push("*** End Patch".to_string());
    lines.join("\n")
}

pub(super) fn compact_subagent_result(tool_name: &str, result: &Value) -> String {
    let summary = result
        .get("summary")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("Sub-agent finished.");
    let mut rendered = match tool_name {
        "spawn_agent" => {
            let name = result
                .get("name")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .unwrap_or("worker");
            format!("spawn_agent {name}: {summary}")
        }
        "explore_agent" => format!("explore_agent {summary}"),
        "plan_agent" => format!("plan_agent {summary}"),
        _ => format!("{tool_name} {summary}"),
    };

    if let Some(agent_id) = result
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!("\nagent_id: {agent_id}"));
    }
    if let Some(session_id) = result
        .get("session_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!("\nsession_id: {session_id}"));
    }
    if let Some(persistence_error) = result
        .get("persistence_error")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!("\npersistence_error: {persistence_error}"));
    }
    append_request_user_input(&mut rendered, result.get("request_user_input"));
    rendered
}

fn append_request_user_input(rendered: &mut String, request: Option<&Value>) {
    let Some(request) = request else {
        return;
    };
    if request.is_null() {
        return;
    }
    if let Some(question) = request
        .get("question")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!("\nrequest_user_input: {question}"));
    }
    if let Some(options) = request.get("options").and_then(Value::as_array) {
        for option in options {
            let Some((label, description)) = parse_request_option(option) else {
                continue;
            };
            rendered.push_str(&format!("\noption: {label} | {description}"));
        }
    }
    if let Some(note) = request
        .get("note")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        rendered.push_str(&format!("\nnote: {note}"));
    }
}

fn parse_request_option(option: &Value) -> Option<(String, String)> {
    if let Some(pair) = option.as_array() {
        let label = pair.first()?.as_str()?.trim();
        let description = pair.get(1).and_then(Value::as_str).unwrap_or("").trim();
        return Some((label.to_string(), description.to_string()));
    }
    if let Some(object) = option.as_object() {
        let label = object
            .get("label")
            .or_else(|| object.get("name"))
            .and_then(Value::as_str)?
            .trim();
        let description = object
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim();
        return Some((label.to_string(), description.to_string()));
    }
    None
}

pub(super) fn summarize_tool_result(tool_name: &str, input: &Value, result: &Value) -> String {
    match tool_name {
        "list_files" => {
            let root = input.get("path").and_then(Value::as_str).unwrap_or(".");
            let total = result
                .get("files")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("Listed {total} path(s) under {root}.")
        }
        "read_file" => {
            let path = input
                .get("path")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let total_chars = result
                .get("content")
                .and_then(Value::as_str)
                .map(|content| content.chars().count())
                .unwrap_or_default();
            let total_lines = result
                .get("total_lines")
                .and_then(Value::as_u64)
                .or_else(|| result.get("observed_lines").and_then(Value::as_u64))
                .unwrap_or_default();
            let total_lines_exact = result
                .get("total_lines_exact")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let start_line = result
                .get("start_line")
                .and_then(Value::as_u64)
                .unwrap_or(1);
            let end_line = result
                .get("end_line")
                .and_then(Value::as_u64)
                .unwrap_or(total_lines);
            let truncated = result
                .get("truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let line_truncated = result
                .get("line_truncated")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            let next_offset = result.get("next_offset").and_then(Value::as_u64);
            let continuation = match (next_offset, line_truncated) {
                (Some(next_offset), true) => {
                    format!(
                        " Truncated line(s); continue with offset={next_offset} for more lines."
                    )
                }
                (Some(next_offset), false) => {
                    format!(" Truncated; continue with offset={next_offset}.")
                }
                (None, true) => " Truncated line(s).".to_string(),
                (None, false) if truncated => " Truncated.".to_string(),
                (None, false) => String::new(),
            };
            let total_label = if total_lines_exact {
                total_lines.to_string()
            } else {
                format!("at least {total_lines}")
            };
            if total_lines > 0 && (start_line != 1 || end_line != total_lines) {
                format!(
                    "Read file {path} lines {start_line}-{end_line} of {total_label} ({total_chars} chars).{continuation}"
                )
            } else {
                format!(
                    "Read file {path} ({total_label} lines, {total_chars} chars).{continuation}"
                )
            }
        }
        "glob" => {
            let total = result
                .get("matches")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("Glob matched {total} path(s).")
        }
        "grep" => {
            let total = result
                .get("results")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            format!("Grep found {total} match(es).")
        }
        "web_fetch" => {
            let url = input
                .get("url")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let total_chars = result
                .get("content")
                .and_then(Value::as_str)
                .map(|content| content.chars().count())
                .unwrap_or_default();
            format!("Fetched {url} ({total_chars} chars).")
        }
        "web_search" => {
            let query = input
                .get("query")
                .and_then(Value::as_str)
                .or_else(|| result.get("query").and_then(Value::as_str))
                .unwrap_or("<unknown>");
            let total_chars = result
                .get("content")
                .and_then(Value::as_str)
                .map(|content| content.chars().count())
                .unwrap_or_default();
            format!("Searched web for {query:?} ({total_chars} chars).")
        }
        "apply_patch" => {
            let status = result
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            let files_changed = result
                .get("files_changed")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let hunks_applied = result
                .get("hunks_applied")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Patch {status}: {files_changed} file(s), {hunks_applied} hunk(s).")
        }
        "write_file" => {
            let path = result
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| input.get("path").and_then(Value::as_str))
                .unwrap_or("<unknown>");
            let operation = result
                .get("operation")
                .and_then(Value::as_str)
                .unwrap_or("updated");
            let line_count = result
                .get("line_count")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            let bytes_written = result
                .get("bytes_written")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Write file {path}: {operation} ({line_count} lines, {bytes_written} bytes).")
        }
        "replace" => {
            let path = result
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| input.get("path").and_then(Value::as_str))
                .unwrap_or("<unknown>");
            let replacements = result
                .get("replacements")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!("Replace in {path}: {replacements} replacement(s).")
        }
        "replace_lines" => {
            let path = result
                .get("path")
                .and_then(Value::as_str)
                .or_else(|| input.get("path").and_then(Value::as_str))
                .unwrap_or("<unknown>");
            let start_line = result
                .get("start_line")
                .and_then(Value::as_u64)
                .or_else(|| input.get("start_line").and_then(Value::as_u64))
                .unwrap_or_default();
            let end_line = result
                .get("end_line")
                .and_then(Value::as_u64)
                .or_else(|| input.get("end_line").and_then(Value::as_u64))
                .unwrap_or_default();
            let inserted_lines = result
                .get("inserted_lines")
                .and_then(Value::as_u64)
                .unwrap_or_default();
            format!(
                "Replaced lines {start_line}-{end_line} in {path}: {inserted_lines} inserted line(s)."
            )
        }
        "lsp_diagnostics" => {
            let file = result
                .get("file")
                .and_then(Value::as_str)
                .unwrap_or("<unknown>");
            let total = result
                .get("diagnostics")
                .and_then(Value::as_array)
                .map_or(0, Vec::len);
            if let Some(error) = result.get("error").and_then(Value::as_str) {
                format!("LSP diagnostics for {file} failed: {error}")
            } else {
                format!("LSP diagnostics for {file}: {total} diagnostic(s).")
            }
        }
        _ => {
            let keys = result
                .as_object()
                .map(|object| object.keys().cloned().collect::<Vec<_>>().join(", "))
                .filter(|keys| !keys.is_empty())
                .unwrap_or_else(|| "scalar result".to_string());
            format!("Tool {tool_name} completed with {keys}.")
        }
    }
}

pub(super) fn compact_lsp_diagnostics(result: &Value) -> String {
    serde_json::to_string_pretty(result).unwrap_or_else(|_| result.to_string())
}

pub(super) fn truncate_text(text: &str, max_chars: usize) -> String {
    let mut collected = String::new();
    for ch in text.chars().take(max_chars) {
        collected.push(ch);
    }
    collected
}

pub(crate) fn model_preview_bash_output(output: &str, exit_code: Option<i64>) -> String {
    let (head_chars, tail_chars) = if exit_code == Some(0) {
        (BASH_SUCCESS_HEAD_CHARS, BASH_SUCCESS_TAIL_CHARS)
    } else {
        (BASH_ERROR_HEAD_CHARS, BASH_ERROR_TAIL_CHARS)
    };
    head_tail_text(output, head_chars, tail_chars)
}

pub(crate) fn head_tail_text(text: &str, head_chars: usize, tail_chars: usize) -> String {
    let budget = head_chars.saturating_add(tail_chars);
    if text.chars().nth(budget).is_none() {
        return text.to_string();
    }

    let head_end = char_boundary_after_n_chars(text, head_chars);
    let tail_start = char_boundary_before_last_n_chars(text, tail_chars);
    let head = &text[..head_end];
    let tail = &text[tail_start..];
    let omitted = text[head_end..tail_start].chars().count();
    format!("{head}\n... [{omitted} chars truncated from middle] ...\n{tail}")
}

fn char_boundary_after_n_chars(text: &str, n: usize) -> usize {
    if n == 0 {
        return 0;
    }
    text.char_indices()
        .nth(n)
        .map(|(idx, _)| idx)
        .unwrap_or(text.len())
}

fn char_boundary_before_last_n_chars(text: &str, n: usize) -> usize {
    if n == 0 {
        return text.len();
    }
    text.char_indices()
        .rev()
        .nth(n.saturating_sub(1))
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}
