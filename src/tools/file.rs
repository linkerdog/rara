use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, BufReader as AsyncBufReader};
use walkdir::WalkDir;

use crate::tool::{Tool, ToolError};

const DEFAULT_READ_LINE_LIMIT: usize = 2_000;
const MAX_READ_LINE_CHARS: usize = 4_000;
const MAX_READ_LINE_BYTES: usize = MAX_READ_LINE_CHARS * 4;

#[derive(Debug, Default)]
pub struct FileReadState {
    files: Mutex<HashMap<PathBuf, FileReadEntry>>,
}

#[derive(Debug)]
struct FileReadEntry {
    modified: SystemTime,
    content: Option<String>,
    is_partial: bool,
}

impl FileReadState {
    pub(crate) fn record_read(
        &self,
        path: &str,
        output: &ReadFileOutput,
        content: Option<String>,
    ) -> Result<(), ToolError> {
        let key = canonical_existing_path(path)?;
        let metadata = fs::metadata(&key)?;
        let modified = metadata.modified()?;
        let is_partial = output.is_partial();
        let mut files = self.files.lock().expect("file read state lock");
        if is_partial {
            if let Some(existing) = files.get(&key) {
                if !existing.is_partial && existing.modified == modified {
                    return Ok(());
                }
            }
        }
        let entry = FileReadEntry {
            modified,
            content,
            is_partial,
        };
        files.insert(key, entry);
        Ok(())
    }

    pub(crate) fn validate_existing_edit(&self, path: &str) -> Result<(), ToolError> {
        let key = canonical_existing_path(path)?;
        let files = self.files.lock().expect("file read state lock");
        let Some(entry) = files.get(&key) else {
            return Err(ToolError::ExecutionFailed(
                "File has not been read yet. Read it first before writing to it.".into(),
            ));
        };
        if entry.is_partial {
            return Err(ToolError::ExecutionFailed(
                "File was only partially read. Read the full file before writing to it.".into(),
            ));
        }

        let metadata = fs::metadata(&key)?;
        let modified = metadata.modified()?;
        if modified > entry.modified {
            if let Some(content) = &entry.content {
                if fs::read_to_string(&key)? == *content {
                    return Ok(());
                }
            }
            return Err(ToolError::ExecutionFailed(
                "File has been modified since read, either by the user or by a formatter. Read it again before attempting to write it.".into(),
            ));
        }

        if let Some(content) = &entry.content {
            let current = fs::read_to_string(&key)?;
            if current != *content {
                return Err(ToolError::ExecutionFailed(
                    "File has changed since read. Read it again before attempting to write it."
                        .into(),
                ));
            }
        }

        Ok(())
    }

    pub(crate) fn validate_exact_replace_edit(&self, path: &str) -> Result<(), ToolError> {
        let key = canonical_existing_path(path)?;
        let files = self.files.lock().expect("file read state lock");
        let Some(entry) = files.get(&key) else {
            return Err(ToolError::ExecutionFailed(
                "File has not been read yet. Read it first before writing to it.".into(),
            ));
        };
        if entry.is_partial {
            return Ok(());
        }
        drop(files);
        self.validate_existing_edit(path)
    }

    pub(crate) fn record_write(&self, path: &str, content: &str) -> Result<(), ToolError> {
        let key = canonical_existing_path(path)?;
        let metadata = fs::metadata(&key)?;
        let entry = FileReadEntry {
            modified: metadata.modified()?,
            content: Some(content.to_string()),
            is_partial: false,
        };
        self.files
            .lock()
            .expect("file read state lock")
            .insert(key, entry);
        Ok(())
    }

    pub(crate) fn forget(&self, path: &str) -> Result<(), ToolError> {
        let key = canonical_existing_path(path)?;
        self.files
            .lock()
            .expect("file read state lock")
            .remove(&key);
        Ok(())
    }
}

pub(crate) type SharedFileReadState = Arc<FileReadState>;

fn canonical_existing_path(path: &str) -> Result<PathBuf, ToolError> {
    Ok(fs::canonicalize(path)?)
}

fn record_write_best_effort(read_state: &FileReadState, path: &str, content: &str) {
    if let Err(err) = read_state.record_write(path, content) {
        eprintln!("Failed to record file read state after write: {err}");
    }
}

pub struct ReadFileTool {
    read_state: Option<SharedFileReadState>,
}

impl ReadFileTool {
    pub fn new(read_state: SharedFileReadState) -> Self {
        Self {
            read_state: Some(read_state),
        }
    }
}

impl Default for ReadFileTool {
    fn default() -> Self {
        Self { read_state: None }
    }
}

#[tool_spec(
    name = "read_file",
    description = "Read a file with Codex-style 1-based offset/limit windows. Defaults to the first 2000 lines and reports next_offset when more content remains.",
    input_schema = {
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to read." },
            "offset": { "type": "integer", "minimum": 1, "description": "Optional 1-based first line to include. Use next_offset from a truncated result to continue." },
            "limit": { "type": "integer", "minimum": 1, "description": "Optional maximum number of lines to return. Defaults to 2000." },
            "start_line": { "type": "integer", "minimum": 1, "description": "Legacy alias for offset." },
            "end_line": { "type": "integer", "minimum": 1, "description": "Legacy 1-based inclusive last line. Prefer offset/limit for new calls." }
        },
        "required": ["path"]
    }
)]
#[async_trait]
impl Tool for ReadFileTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["path"]
            .as_str()
            .ok_or(ToolError::InvalidInput("path".into()))?;
        let window = read_file_window_from_input(&i)?;
        let output = read_file_window(p, window).await?;
        if let Some(read_state) = &self.read_state {
            let full_content =
                (!output.truncated && output.start_line == 1 && output.total_lines_exact)
                    .then(|| fs::read_to_string(p))
                    .transpose()?;
            read_state.record_read(p, &output, full_content)?;
        }

        Ok(json!({
            "content": output.content,
            "total_lines": output.total_lines,
            "total_lines_exact": output.total_lines_exact,
            "observed_lines": output.observed_lines,
            "start_line": output.start_line,
            "end_line": output.end_line,
            "offset": output.start_line,
            "limit": output.limit,
            "num_lines": output.num_lines,
            "truncated": output.truncated,
            "next_offset": output.next_offset,
            "line_truncated": output.line_truncated,
            "line_format": "raw",
            "bytes_read": output.bytes_read,
        }))
    }
}

#[derive(Clone, Copy)]
struct ReadFileWindow {
    offset: usize,
    limit: usize,
}

pub(crate) struct ReadFileOutput {
    content: String,
    total_lines: Option<usize>,
    total_lines_exact: bool,
    observed_lines: usize,
    start_line: usize,
    end_line: usize,
    limit: usize,
    num_lines: usize,
    truncated: bool,
    next_offset: Option<usize>,
    line_truncated: bool,
    bytes_read: usize,
}

impl ReadFileOutput {
    fn is_partial(&self) -> bool {
        self.truncated || self.start_line != 1 || !self.total_lines_exact
    }
}

struct BoundedLineRead {
    bytes_read: usize,
    eof: bool,
    truncated: bool,
}

fn read_file_window_from_input(input: &Value) -> Result<ReadFileWindow, ToolError> {
    let offset = optional_positive_usize(input, "offset")?;
    let limit = optional_positive_usize(input, "limit")?;
    let start_line = optional_positive_usize(input, "start_line")?;
    let end_line = optional_positive_usize(input, "end_line")?;

    if (offset.is_some() || limit.is_some()) && (start_line.is_some() || end_line.is_some()) {
        return Err(ToolError::InvalidInput(
            "Use either offset/limit or start_line/end_line, not both".into(),
        ));
    }

    if let Some(offset) = offset {
        return Ok(ReadFileWindow {
            offset,
            limit: limit.unwrap_or(DEFAULT_READ_LINE_LIMIT),
        });
    }

    if let Some(limit) = limit {
        return Ok(ReadFileWindow { offset: 1, limit });
    }

    if start_line.is_some() || end_line.is_some() {
        let start = start_line.unwrap_or(1);
        let limit = match end_line {
            Some(end) if start > end => {
                return Err(ToolError::InvalidInput(
                    "start_line must be <= end_line".into(),
                ));
            }
            Some(end) => end - start + 1,
            None => DEFAULT_READ_LINE_LIMIT,
        };
        return Ok(ReadFileWindow {
            offset: start,
            limit,
        });
    }

    Ok(ReadFileWindow {
        offset: 1,
        limit: DEFAULT_READ_LINE_LIMIT,
    })
}

fn optional_positive_usize(input: &Value, key: &str) -> Result<Option<usize>, ToolError> {
    let Some(value) = input.get(key) else {
        return Ok(None);
    };
    let Some(value) = value.as_u64() else {
        return Err(ToolError::InvalidInput(format!("{key} must be an integer")));
    };
    if value == 0 {
        return Err(ToolError::InvalidInput(format!("{key} must be >= 1")));
    }
    Ok(Some(value as usize))
}

async fn read_file_window(path: &str, window: ReadFileWindow) -> Result<ReadFileOutput, ToolError> {
    let file = tokio::fs::File::open(path).await?;
    let mut reader = AsyncBufReader::new(file);
    let mut line = String::new();
    let mut observed_lines = 0usize;
    let mut total_lines_exact = false;
    let mut selected = Vec::new();
    let mut line_truncated = false;
    let mut bytes_read = 0usize;
    let last_requested_line = window
        .offset
        .checked_add(window.limit - 1)
        .ok_or_else(|| ToolError::InvalidInput("offset + limit overflows".into()))?;

    loop {
        line.clear();
        let read = read_bounded_line(&mut reader, &mut line).await?;
        if read.eof {
            total_lines_exact = true;
            break;
        }
        let bytes = read.bytes_read;
        bytes_read += bytes;
        observed_lines += 1;
        line_truncated |= read.truncated;

        if observed_lines > last_requested_line {
            break;
        }

        if observed_lines < window.offset {
            continue;
        }

        let text = line.trim_end_matches(['\r', '\n']);
        let (text, truncated) = truncate_read_line(text);
        line_truncated |= truncated;
        selected.push(text);
    }

    if window.offset > observed_lines.max(1) {
        return Err(ToolError::ExecutionFailed(format!(
            "offset {} exceeds file length {}",
            window.offset, observed_lines
        )));
    }

    let num_lines = selected.len();
    let end_line = if num_lines == 0 {
        0
    } else {
        window.offset + num_lines - 1
    };
    let has_more_lines = observed_lines > end_line;
    let next_offset = if has_more_lines {
        Some(end_line + 1)
    } else {
        None
    };

    Ok(ReadFileOutput {
        content: selected.join("\n"),
        total_lines: total_lines_exact.then_some(observed_lines),
        total_lines_exact,
        observed_lines,
        start_line: window.offset,
        end_line,
        limit: window.limit,
        num_lines,
        truncated: has_more_lines || line_truncated,
        next_offset,
        line_truncated,
        bytes_read,
    })
}

async fn read_bounded_line<R>(
    reader: &mut R,
    line: &mut String,
) -> Result<BoundedLineRead, ToolError>
where
    R: AsyncBufRead + Unpin,
{
    let mut captured = Vec::new();
    let mut bytes_read = 0usize;
    let mut truncated = false;
    line.clear();

    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            *line = String::from_utf8_lossy(&captured).into_owned();
            return Ok(BoundedLineRead {
                bytes_read,
                eof: bytes_read == 0,
                truncated,
            });
        }

        let newline_pos = available.iter().position(|byte| *byte == b'\n');
        let chunk_len = newline_pos.map_or(available.len(), |pos| pos + 1);
        let remaining = MAX_READ_LINE_BYTES.saturating_sub(captured.len());
        if remaining > 0 {
            let take = remaining.min(chunk_len);
            captured.extend_from_slice(&available[..take]);
            truncated |= take < chunk_len;
        } else {
            truncated = true;
        }
        bytes_read += chunk_len;
        reader.consume(chunk_len);

        if newline_pos.is_some() {
            *line = String::from_utf8_lossy(&captured).into_owned();
            return Ok(BoundedLineRead {
                bytes_read,
                eof: false,
                truncated,
            });
        }
    }
}

fn truncate_read_line(line: &str) -> (String, bool) {
    if line.chars().count() <= MAX_READ_LINE_CHARS {
        return (line.to_string(), false);
    }

    let mut truncated = line.chars().take(MAX_READ_LINE_CHARS).collect::<String>();
    truncated.push_str("... [line truncated]");
    (truncated, true)
}

pub struct WriteFileTool {
    read_state: Option<SharedFileReadState>,
}

impl WriteFileTool {
    pub fn new(read_state: SharedFileReadState) -> Self {
        Self {
            read_state: Some(read_state),
        }
    }
}

impl Default for WriteFileTool {
    fn default() -> Self {
        Self { read_state: None }
    }
}

#[tool_spec(
    name = "write_file",
    description = "Create a new file or intentionally rewrite one whole file. For existing files, read the full file first and prefer apply_patch for partial edits. If a large write fails or appears truncated, retry with direct edit tools or report the tool failure; do not fall back to shell heredocs or redirection to write files.",
    input_schema = {
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to create or fully rewrite." },
            "content": { "type": "string", "description": "Complete new file contents for exactly this path. Do not use for small edits to existing files or shell heredoc fallbacks." }
        },
        "required": ["path", "content"]
    }
)]
#[async_trait]
impl Tool for WriteFileTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["path"]
            .as_str()
            .ok_or(ToolError::InvalidInput("path".into()))?;
        let c = i["content"]
            .as_str()
            .ok_or(ToolError::InvalidInput("content".into()))?;
        let existing = existing_file_summary(p)?;
        if existing.is_some()
            && let Some(read_state) = &self.read_state
        {
            read_state.validate_existing_edit(p)?;
        }
        let operation = if existing.is_some() {
            "overwritten"
        } else {
            "created"
        };
        fs::write(p, c)?;
        if let Some(read_state) = &self.read_state {
            record_write_best_effort(read_state, p, c);
        }
        Ok(json!({
            "status": "ok",
            "path": p,
            "operation": operation,
            "bytes_written": c.len(),
            "line_count": c.lines().count(),
            "previous_bytes": existing.as_ref().map(|(bytes, _)| *bytes),
            "previous_line_count": existing.as_ref().map(|(_, line_count)| *line_count),
        }))
    }
}

pub struct ReplaceTool {
    read_state: Option<SharedFileReadState>,
}

impl ReplaceTool {
    pub fn new(read_state: SharedFileReadState) -> Self {
        Self {
            read_state: Some(read_state),
        }
    }
}

impl Default for ReplaceTool {
    fn default() -> Self {
        Self { read_state: None }
    }
}

#[tool_spec(
    name = "replace",
    description = "Replace one exact, unique string in a file. Read the relevant file content first and prefer apply_patch for structured multi-line edits.",
    input_schema = {
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to edit." },
            "old_string": { "type": "string", "description": "Exact text to replace. It must appear exactly once in the file." },
            "new_string": { "type": "string", "description": "Replacement text." }
        },
        "required": ["path", "old_string", "new_string"]
    }
)]
#[async_trait]
impl Tool for ReplaceTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["path"]
            .as_str()
            .ok_or(ToolError::InvalidInput("path".into()))?;
        let o = i["old_string"]
            .as_str()
            .ok_or(ToolError::InvalidInput("old".into()))?;
        let n = i["new_string"]
            .as_str()
            .ok_or(ToolError::InvalidInput("new".into()))?;
        if let Some(read_state) = &self.read_state {
            read_state.validate_exact_replace_edit(p)?;
        }
        let c = fs::read_to_string(p)?;
        if c.matches(o).count() != 1 {
            return Err(ToolError::ExecutionFailed("String not unique".into()));
        }
        let updated = c.replace(o, n);
        fs::write(p, &updated)?;
        if let Some(read_state) = &self.read_state {
            record_write_best_effort(read_state, p, &updated);
        }
        Ok(json!({
            "status": "ok",
            "path": p,
            "replacements": 1,
            "old_preview": preview_snippet(o),
            "new_preview": preview_snippet(n),
            "old_bytes": o.len(),
            "new_bytes": n.len(),
            "line_delta": updated.lines().count() as i64 - c.lines().count() as i64,
        }))
    }
}

pub struct ReplaceLinesTool {
    read_state: Option<SharedFileReadState>,
}

impl ReplaceLinesTool {
    pub fn new(read_state: SharedFileReadState) -> Self {
        Self {
            read_state: Some(read_state),
        }
    }
}

impl Default for ReplaceLinesTool {
    fn default() -> Self {
        Self { read_state: None }
    }
}

#[tool_spec(
    name = "replace_lines",
    description = "Replace an inclusive line range in a file. Use only after reading the full file and verifying the current line numbers.",
    input_schema = {
        "type": "object",
        "properties": {
            "path": { "type": "string", "description": "Path to edit." },
            "start_line": { "type": "integer", "minimum": 1, "description": "1-based first line to replace." },
            "end_line": { "type": "integer", "minimum": 1, "description": "1-based last line to replace, inclusive." },
            "new_string": {
                "type": "string",
                "description": "Replacement text for the inclusive line range. Use an empty string to delete the range."
            }
        },
        "required": ["path", "start_line", "end_line", "new_string"]
    }
)]
#[async_trait]
impl Tool for ReplaceLinesTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let path = i["path"]
            .as_str()
            .ok_or(ToolError::InvalidInput("path".into()))?;
        let start_line = i["start_line"]
            .as_u64()
            .ok_or(ToolError::InvalidInput("start_line".into()))? as usize;
        let end_line = i["end_line"]
            .as_u64()
            .ok_or(ToolError::InvalidInput("end_line".into()))? as usize;
        let new_string = i["new_string"]
            .as_str()
            .ok_or(ToolError::InvalidInput("new_string".into()))?;
        if start_line == 0 || end_line == 0 {
            return Err(ToolError::InvalidInput(
                "start_line/end_line must be >= 1".into(),
            ));
        }
        if start_line > end_line {
            return Err(ToolError::InvalidInput(
                "start_line must be <= end_line".into(),
            ));
        }
        if let Some(read_state) = &self.read_state {
            read_state.validate_existing_edit(path)?;
        }

        let original = fs::read_to_string(path)?;
        let had_trailing_newline = original.ends_with('\n');
        let mut lines = original
            .lines()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        let total_lines = lines.len();
        if total_lines == 0 {
            return Err(ToolError::ExecutionFailed(format!(
                "Cannot replace lines in empty file {path}"
            )));
        }
        if end_line > total_lines {
            return Err(ToolError::ExecutionFailed(format!(
                "Line range {start_line}-{end_line} exceeds file length {total_lines}"
            )));
        }

        let replacement_lines = if new_string.is_empty() {
            Vec::new()
        } else {
            new_string
                .lines()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        };
        let removed_line_count = end_line - start_line + 1;
        let removed_string = lines[start_line - 1..end_line].join("\n");
        lines.splice(start_line - 1..end_line, replacement_lines.iter().cloned());

        let mut updated = lines.join("\n");
        if had_trailing_newline && !updated.is_empty() {
            updated.push('\n');
        }
        fs::write(path, &updated)?;
        if let Some(read_state) = &self.read_state {
            record_write_best_effort(read_state, path, &updated);
        }

        Ok(json!({
            "status": "ok",
            "path": path,
            "start_line": start_line,
            "end_line": end_line,
            "removed_lines": removed_line_count,
            "removed_string": removed_string,
            "inserted_lines": replacement_lines.len(),
            "line_delta": replacement_lines.len() as i64 - removed_line_count as i64,
        }))
    }
}

pub struct ListFilesTool;
#[tool_spec(
    name = "list_files",
    description = "Recursively list files",
    input_schema = {
        "type": "object",
        "properties": {
            "path": { "type": "string" },
            "include_ignored": { "type": "boolean" }
        },
        "required": ["path"]
    }
)]
#[async_trait]
impl Tool for ListFilesTool {
    async fn call(&self, i: Value) -> Result<Value, ToolError> {
        let p = i["path"]
            .as_str()
            .ok_or(ToolError::InvalidInput("path".into()))?;
        let include_ignored = i
            .get("include_ignored")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let files: Vec<String> = WalkDir::new(p)
            .into_iter()
            .filter_entry(|entry| include_ignored || !is_ignored_path(entry.path()))
            .filter_map(|e| e.ok())
            .map(|e| e.path().display().to_string())
            .collect();
        Ok(json!({ "files": files }))
    }
}

fn is_ignored_path(path: &Path) -> bool {
    path.components().any(|component| {
        let name = component.as_os_str().to_string_lossy();
        matches!(
            name.as_ref(),
            ".git"
                | "target"
                | "node_modules"
                | "dist"
                | "build"
                | ".next"
                | ".cache"
                | "__pycache__"
                | ".venv"
                | "venv"
        )
    })
}

fn preview_snippet(value: &str) -> String {
    let mut preview = value.replace('\n', "\\n");
    const MAX_PREVIEW: usize = 80;
    if preview.chars().count() > MAX_PREVIEW {
        preview = preview.chars().take(MAX_PREVIEW).collect::<String>();
        preview.push_str("...");
    }
    preview
}

fn existing_file_summary(path: &str) -> Result<Option<(u64, usize)>, ToolError> {
    let metadata = match fs::metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(ToolError::Io(err)),
    };

    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut line_count = 0usize;
    for line in reader.lines() {
        line?;
        line_count += 1;
    }

    Ok(Some((metadata.len(), line_count)))
}

#[cfg(test)]
mod tests;
