use std::fs;
use std::path::Path;

use async_trait::async_trait;
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::tool::{Tool, ToolError};
use crate::tools::file::{FileReadState, SharedFileReadState};

#[derive(Default)]
pub struct ApplyPatchTool {
    read_state: Option<SharedFileReadState>,
}

impl ApplyPatchTool {
    pub fn new(read_state: SharedFileReadState) -> Self {
        Self {
            read_state: Some(read_state),
        }
    }
}

const PATCH_PREVIEW_LINE_LIMIT: usize = 120;

#[derive(Debug)]
enum PatchOp {
    Add {
        path: String,
        lines: Vec<String>,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        chunks: Vec<Chunk>,
    },
}

#[derive(Debug)]
struct Chunk {
    lines: Vec<DiffLine>,
}

#[derive(Debug)]
struct DiffLine {
    kind: DiffLineKind,
    text: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DiffLineKind {
    Context,
    Addition,
    Removal,
}

impl DiffLineKind {
    fn from_marker(marker: char) -> Option<Self> {
        match marker {
            ' ' => Some(Self::Context),
            '+' => Some(Self::Addition),
            '-' => Some(Self::Removal),
            _ => None,
        }
    }
}

#[derive(Default)]
struct PatchStats {
    files_changed: usize,
    hunks_applied: usize,
    created_files: Vec<String>,
    deleted_files: Vec<String>,
    moved_files: Vec<Value>,
    updated_files: Vec<String>,
    added_lines: usize,
    removed_lines: usize,
}

#[tool_spec(
    name = "apply_patch",
    description = "Apply structured file edits using Begin Patch syntax. Prefer this for editing existing files. Update operations verify hunks against current file contents.",
    input_schema = {
        "type": "object",
        "properties": {
            "patch": {
                "type": "string",
                "description": "Patch text using *** Begin Patch / *** End Patch syntax."
            },
            "dry_run": {
                "type": "boolean",
                "description": "Validate and preview without writing files."
            }
        },
        "required": ["patch"]
    }
)]
#[async_trait]
impl Tool for ApplyPatchTool {
    async fn call(&self, input: Value) -> Result<Value, ToolError> {
        let patch = input
            .get("patch")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::InvalidInput("patch".into()))?;
        let dry_run = input
            .get("dry_run")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        let ops = parse_patch(patch)?;
        validate_patch_update_context(&ops)?;
        let mut stats = PatchStats::default();
        let mut previews = Vec::new();

        for op in ops {
            match op {
                PatchOp::Add { path, lines } => {
                    if Path::new(&path).exists() {
                        return Err(ToolError::ExecutionFailed(format!(
                            "Cannot add existing file {path}"
                        )));
                    }
                    stats.files_changed += 1;
                    stats.created_files.push(path.clone());
                    stats.hunks_applied += 1;
                    stats.added_lines += lines.len();
                    previews.push(format!("Add file {path}"));
                    if !dry_run {
                        let content = join_lines(&lines);
                        write_text_file(&path, &content)?;
                        if let Some(read_state) = &self.read_state {
                            record_patch_write_best_effort(read_state, &path, &content);
                        }
                    }
                }
                PatchOp::Delete { path } => {
                    if !Path::new(&path).exists() {
                        return Err(ToolError::ExecutionFailed(format!(
                            "Cannot delete missing file {path}"
                        )));
                    }
                    let removed_lines = read_lines(&path)?.len();
                    stats.files_changed += 1;
                    stats.deleted_files.push(path.clone());
                    stats.hunks_applied += 1;
                    stats.removed_lines += removed_lines;
                    previews.push(format!("Delete file {path}"));
                    if !dry_run {
                        if let Some(read_state) = &self.read_state {
                            read_state.forget(&path)?;
                        }
                        fs::remove_file(&path)?;
                    }
                }
                PatchOp::Update {
                    path,
                    move_to,
                    chunks,
                } => {
                    let original = fs::read_to_string(&path)?;
                    let updated = apply_update_chunks(&path, &original, &chunks, &mut stats)?;
                    stats.files_changed += 1;
                    stats.updated_files.push(path.clone());
                    previews.push(format!(
                        "Update file {}{}",
                        path,
                        move_to
                            .as_ref()
                            .map(|target| format!(" -> {target}"))
                            .unwrap_or_default()
                    ));
                    if let Some(target) = &move_to {
                        stats
                            .moved_files
                            .push(json!({ "from": path, "to": target }));
                    }
                    if !dry_run {
                        let write_path = move_to.as_deref().unwrap_or(&path);
                        if let Some(target) = &move_to
                            && target != &path
                        {
                            if let Some(parent) = Path::new(target).parent() {
                                fs::create_dir_all(parent)?;
                            }
                            if let Some(read_state) = &self.read_state {
                                read_state.forget(&path)?;
                            }
                            fs::remove_file(&path)?;
                        }
                        write_text_file(write_path, &updated)?;
                        if let Some(read_state) = &self.read_state {
                            record_patch_write_best_effort(read_state, write_path, &updated);
                        }
                    }
                }
            }
        }

        let (diff_preview, diff_truncated) = patch_preview(patch);

        Ok(json!({
            "status": if dry_run { "validated" } else { "applied" },
            "files_changed": stats.files_changed,
            "hunks_applied": stats.hunks_applied,
            "created_files": stats.created_files,
            "deleted_files": stats.deleted_files,
            "moved_files": stats.moved_files,
            "updated_files": stats.updated_files,
            "line_delta": {
                "added": stats.added_lines,
                "removed": stats.removed_lines,
            },
            "summary": previews,
            "diff_preview": diff_preview,
            "diff_truncated": diff_truncated,
        }))
    }
}

fn validate_patch_update_context(ops: &[PatchOp]) -> Result<(), ToolError> {
    for op in ops {
        if let PatchOp::Update { path, chunks, .. } = op {
            if chunks.is_empty() {
                return Err(ToolError::ExecutionFailed(format!(
                    "Update patch for {path} must include at least one hunk"
                )));
            }
            for chunk in chunks {
                if chunk
                    .lines
                    .iter()
                    .all(|line| line.kind == DiffLineKind::Addition)
                {
                    return Err(ToolError::ExecutionFailed(format!(
                        "Patch hunk for {path} must include at least one context or removed line"
                    )));
                }
            }
        }
    }
    Ok(())
}

fn record_patch_write_best_effort(read_state: &FileReadState, path: &str, content: &str) {
    if let Err(err) = read_state.record_write(path, content) {
        eprintln!("Failed to record file read state after patch write: {err}");
    }
}

fn patch_preview(patch: &str) -> (String, bool) {
    let lines = patch
        .lines()
        .take(PATCH_PREVIEW_LINE_LIMIT)
        .collect::<Vec<_>>();
    let truncated = patch.lines().nth(PATCH_PREVIEW_LINE_LIMIT).is_some();
    let mut preview = lines.join("\n");
    if truncated {
        preview.push_str("\n... diff truncated");
    }
    (preview, truncated)
}

fn parse_patch(patch: &str) -> Result<Vec<PatchOp>, ToolError> {
    let lines: Vec<&str> = patch.lines().collect();
    if lines.first().copied() != Some("*** Begin Patch") {
        return Err(ToolError::InvalidInput(
            "Patch must start with *** Begin Patch".into(),
        ));
    }
    if lines.last().copied() != Some("*** End Patch") {
        return Err(ToolError::InvalidInput(
            "Patch must end with *** End Patch".into(),
        ));
    }

    let mut ops = Vec::new();
    let mut index = 1usize;
    while index + 1 < lines.len() {
        let line = lines[index];
        if let Some(path) = line.strip_prefix("*** Add File: ") {
            index += 1;
            let mut add_lines = Vec::new();
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let content = lines[index];
                let Some(text) = content.strip_prefix('+') else {
                    return Err(ToolError::InvalidInput(format!(
                        "Add file entries must start with '+': {content}"
                    )));
                };
                add_lines.push(text.to_string());
                index += 1;
            }
            ops.push(PatchOp::Add {
                path: path.to_string(),
                lines: add_lines,
            });
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Delete File: ") {
            ops.push(PatchOp::Delete {
                path: path.to_string(),
            });
            index += 1;
            continue;
        }

        if let Some(path) = line.strip_prefix("*** Update File: ") {
            index += 1;
            let mut move_to = None;
            if index < lines.len()
                && let Some(target) = lines[index].strip_prefix("*** Move to: ")
            {
                move_to = Some(target.to_string());
                index += 1;
            }

            let mut chunks = Vec::new();
            let mut current_chunk: Option<Chunk> = None;
            while index < lines.len() && !lines[index].starts_with("*** ") {
                let current = lines[index];
                if current.starts_with("@@") {
                    if let Some(chunk) = current_chunk.take() {
                        chunks.push(chunk);
                    }
                    current_chunk = Some(Chunk { lines: Vec::new() });
                } else if current == "*** End of File" {
                } else {
                    let kind = current.chars().next().ok_or_else(|| {
                        ToolError::InvalidInput("Unexpected empty patch line".into())
                    })?;
                    let Some(kind) = DiffLineKind::from_marker(kind) else {
                        return Err(ToolError::InvalidInput(format!(
                            "Unexpected patch line: {current}"
                        )));
                    };
                    let chunk = current_chunk.get_or_insert_with(|| Chunk { lines: Vec::new() });
                    chunk.lines.push(DiffLine {
                        kind,
                        text: current[1..].to_string(),
                    });
                }
                index += 1;
            }
            if let Some(chunk) = current_chunk.take() {
                chunks.push(chunk);
            }
            ops.push(PatchOp::Update {
                path: path.to_string(),
                move_to,
                chunks,
            });
            continue;
        }

        return Err(ToolError::InvalidInput(format!(
            "Unexpected patch directive: {line}"
        )));
    }

    Ok(ops)
}

fn apply_update_chunks(
    path: &str,
    original: &str,
    chunks: &[Chunk],
    stats: &mut PatchStats,
) -> Result<String, ToolError> {
    let original_lines = split_lines(original);

    // Phase 1: resolve every hunk to a concrete replacement position.
    // Hunks must appear in file order; each search starts after the
    // previous match so that the same context line is not claimed twice.
    let mut replacements: Vec<(usize, usize, Vec<String>)> = Vec::new();
    let mut cursor = 0usize;

    for chunk in chunks {
        let mut old_lines = Vec::new();
        let mut added_in_chunk = 0usize;
        let mut removed_in_chunk = 0usize;

        let mut new_lines: Vec<String> = Vec::new();
        for line in &chunk.lines {
            match line.kind {
                DiffLineKind::Context => {
                    old_lines.push(line.text.clone());
                    new_lines.push(line.text.clone());
                }
                DiffLineKind::Addition => {
                    new_lines.push(line.text.clone());
                    added_in_chunk += 1;
                }
                DiffLineKind::Removal => {
                    old_lines.push(line.text.clone());
                    removed_in_chunk += 1;
                }
            }
        }

        let Some(pos) = seek_sequence(&original_lines, &old_lines, cursor, false) else {
            return Err(ToolError::ExecutionFailed(format!(
                "Patch hunk did not match file {path}"
            )));
        };
        replacements.push((pos, old_lines.len(), new_lines));
        cursor = pos + old_lines.len();
        stats.hunks_applied += 1;
        stats.added_lines += added_in_chunk;
        stats.removed_lines += removed_in_chunk;
    }

    // Phase 2: apply from tail to head so earlier positions stay valid.
    let mut output = original_lines;
    // Sort ascending by position, then iterate in reverse.
    replacements.sort_by_key(|(pos, _, _)| *pos);
    for (pos, old_len, new_lines) in replacements.into_iter().rev() {
        let end = pos + old_len;
        output.splice(pos..end, new_lines);
    }

    Ok(join_lines(&output))
}

fn split_lines(text: &str) -> Vec<String> {
    text.lines().map(str::to_string).collect()
}

fn join_lines(lines: &[String]) -> String {
    if lines.is_empty() {
        String::new()
    } else {
        format!("{}\n", lines.join("\n"))
    }
}

/// Attempt to locate `pattern` within `lines` starting at or after `start`.
///
/// Matches are attempted with decreasing strictness, following the Codex
/// `seek_sequence` contract:
///   1. Exact match
///   2. Trailing-whitespace-insensitive (rstrip)
///   3. Leading-and-trailing-whitespace-insensitive (trim)
///   4. Unicode-normalised (fancy quotes / dashes / spaces → ASCII)
///
/// When `eof` is true the search starts from the end-of-file so that
/// patterns intended to match file endings are applied there first.
fn seek_sequence(lines: &[String], pattern: &[String], start: usize, eof: bool) -> Option<usize> {
    if pattern.is_empty() {
        return Some(start);
    }
    if pattern.len() > lines.len() {
        return None;
    }
    let search_start = if eof && lines.len() >= pattern.len() {
        lines.len() - pattern.len()
    } else {
        start
    };

    // 1. Exact match
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()] == *pattern {
            return Some(i);
        }
    }

    // 2. Trailing-whitespace-insensitive
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(a, b)| a.trim_end() == b.trim_end())
        {
            return Some(i);
        }
    }

    // 3. Full-trim
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(pattern)
            .all(|(a, b)| a.trim() == b.trim())
        {
            return Some(i);
        }
    }

    // 4. Unicode-normalised (fancy punctuation → ASCII, mirroring git apply)
    let npattern: Vec<String> = pattern.iter().map(|s| normalise_unicode(s)).collect();
    for i in search_start..=lines.len().saturating_sub(pattern.len()) {
        if lines[i..i + pattern.len()]
            .iter()
            .zip(&npattern)
            .all(|(a, b)| normalise_unicode(a) == *b)
        {
            return Some(i);
        }
    }

    None
}

/// Normalise common Unicode punctuation to ASCII equivalents so that diffs
/// authored with plain ASCII characters can still be applied to source files
/// that contain typographic dashes, quotes, and spaces.
fn normalise_unicode(s: &str) -> String {
    s.trim()
        .chars()
        .map(|c| match c {
            // Various dash / hyphen code-points → ASCII '-'
            '\u{2010}' | '\u{2011}' | '\u{2012}' | '\u{2013}' | '\u{2014}' | '\u{2015}'
            | '\u{2212}' => '-',
            // Fancy single quotes → '\''
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            // Fancy double quotes → '"'
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            // Non-breaking / odd spaces → normal space
            '\u{00A0}' | '\u{2002}' | '\u{2003}' | '\u{2004}' | '\u{2005}' | '\u{2006}'
            | '\u{2007}' | '\u{2008}' | '\u{2009}' | '\u{200A}' | '\u{202F}' | '\u{205F}'
            | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}

fn find_subsequence(haystack: &[String], needle: &[String]) -> Option<usize> {
    seek_sequence(haystack, needle, 0, false)
}

fn write_text_file(path: &str, content: &str) -> Result<(), ToolError> {
    if let Some(parent) = Path::new(path).parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, content)?;
    Ok(())
}

fn read_lines(path: &str) -> Result<Vec<String>, ToolError> {
    Ok(split_lines(&fs::read_to_string(path)?))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::ApplyPatchTool;
    use crate::tool::Tool;
    use crate::tools::file::{FileReadState, ReadFileTool};

    #[tokio::test]
    async fn applies_update_patch() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");

        let tool = ApplyPatchTool::default();
        let result = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
                    file.display()
                )
            }))
            .await
            .expect("apply patch");

        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hi\nworld\n");
        assert_eq!(result["status"], "applied");
        assert_eq!(result["files_changed"], 1);
        assert!(
            result["diff_preview"]
                .as_str()
                .expect("diff preview")
                .contains("-hello\n+hi")
        );
    }

    #[tokio::test]
    async fn supports_dry_run() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");

        let tool = ApplyPatchTool::default();
        let result = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
                    file.display()
                ),
                "dry_run": true
            }))
            .await
            .expect("validate patch");

        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "hello\nworld\n"
        );
        assert_eq!(result["status"], "validated");
    }

    #[tokio::test]
    async fn creates_new_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("created.txt");

        let tool = ApplyPatchTool::default();
        let result = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Add File: {}\n+hello\n+world\n*** End Patch",
                    file.display()
                )
            }))
            .await
            .expect("create file");

        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "hello\nworld\n"
        );
        assert_eq!(result["status"], "applied");
        assert_eq!(result["created_files"][0], file.display().to_string());
    }

    #[tokio::test]
    async fn update_patch_allows_partial_read_when_hunk_matches_current_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");
        let read_state = Arc::new(FileReadState::default());
        let read_tool = ReadFileTool::new(read_state.clone());
        let patch_tool = ApplyPatchTool::new(read_state.clone());

        read_tool
            .call(json!({
                "path": file.display().to_string(),
                "offset": 1,
                "limit": 1
            }))
            .await
            .expect("partial read");
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
            file.display()
        );
        patch_tool
            .call(json!({ "patch": patch }))
            .await
            .expect("patch after partial read");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hi\nworld\n");
    }

    #[tokio::test]
    async fn update_patch_rejects_add_only_hunk_without_current_context() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\n").expect("write");
        let tool = ApplyPatchTool::default();

        let error = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n+inserted\n*** End Patch",
                    file.display()
                )
            }))
            .await
            .expect_err("add-only update hunk should be rejected");

        assert!(
            error
                .to_string()
                .contains("must include at least one context or removed line")
        );
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hello\n");
    }

    #[tokio::test]
    async fn update_patch_rejects_empty_hunks() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        let moved = dir.path().join("moved.txt");
        std::fs::write(&file, "hello\n").expect("write");
        let tool = ApplyPatchTool::default();

        let error = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n*** Move to: {}\n*** End Patch",
                    file.display(),
                    moved.display()
                )
            }))
            .await
            .expect_err("empty update hunk should be rejected");

        assert!(error.to_string().contains("must include at least one hunk"));
        assert!(file.exists());
        assert!(!moved.exists());
    }

    #[tokio::test]
    async fn update_patch_tolerates_trailing_whitespace() {
        // Model outputs often trim trailing whitespace, but the file may
        // have spaces at end-of-line. seek_sequence level 2 handles this.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello  \nworld\n").expect("write");

        let tool = ApplyPatchTool::default();
        let result = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
                    file.display()
                )
            }))
            .await
            .expect("apply patch with trailing space");

        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hi\nworld\n");
        assert_eq!(result["status"], "applied");
    }

    #[tokio::test]
    async fn update_patch_tolerates_unicode_fancy_quotes() {
        // Model outputs often use plain ASCII quotes, but the file may
        // contain typographic quotes. seek_sequence level 4 handles this.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "\u{201C}hello\u{201D}\nworld\n").expect("write");

        let tool = ApplyPatchTool::default();
        let result = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Update File: {}\n@@\n-\"hello\"\n+hi\n world\n*** End Patch",
                    file.display()
                )
            }))
            .await
            .expect("apply patch with fancy quotes");

        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hi\nworld\n");
        assert_eq!(result["status"], "applied");
    }

    #[tokio::test]
    async fn delete_patch_allows_partial_read_when_state_is_enabled() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");
        let read_state = Arc::new(FileReadState::default());
        let read_tool = ReadFileTool::new(read_state.clone());
        let patch_tool = ApplyPatchTool::new(read_state);

        read_tool
            .call(json!({
                "path": file.display().to_string(),
                "offset": 1,
                "limit": 1
            }))
            .await
            .expect("partial read");

        let result = patch_tool
            .call(json!({
                "patch": format!("*** Begin Patch\n*** Delete File: {}\n*** End Patch", file.display())
            }))
            .await
            .expect("delete after partial read");

        assert_eq!(result["status"], "applied");
        assert_eq!(result["files_changed"], 1);
        assert_eq!(result["deleted_files"][0], file.display().to_string());
        assert!(!file.exists());
    }
}
