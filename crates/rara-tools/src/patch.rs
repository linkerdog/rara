use std::fs;
use std::path::Path;

use async_trait::async_trait;
use rara_apply_patch::{
    AppliedPatchChange, AppliedPatchDelta, AppliedPatchFileChange, PatchChange, PatchError,
    PatchOp, build_patch_action_from_ops, parse_patch, validate_patch_update_context,
};
use rara_tool_macros::tool_spec;
use serde_json::{Value, json};

use crate::file::{FileReadState, SharedFileReadState, read_file_content};
use crate::tool::{Tool, ToolError};

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

#[tool_spec(
    name = "apply_patch",
    description = "Apply structured file edits using Begin Patch syntax. Prefer this for editing existing files and for related edits across multiple locations. Use this instead of shell sed, awk, perl, redirection, heredocs, or ad-hoc scripts for reviewable file edits. Update operations verify hunks against current file contents.",
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

        // Pre-read enforcement: every Update or Delete must target a file
        // that was fully read in this conversation and hasn't been modified
        // since.  Add ops (new files) are exempt.
        if !dry_run && let Some(read_state) = &self.read_state {
            for op in &ops {
                match op {
                    PatchOp::Update { path, .. } | PatchOp::Delete { path } => {
                        read_state.validate_existing_edit(path)?;
                    }
                    _ => {}
                }
            }
        }

        let mut read_for_action = |path: &str| match read_file_content(path) {
            Ok(content) => Ok(Some(content)),
            Err(err) => match err {
                ToolError::Io(io) if io.kind() == std::io::ErrorKind::NotFound => Ok(None),
                other => Err(PatchError::ExecutionFailed(other.to_string())),
            },
        };
        let action = build_patch_action_from_ops(patch, ops, &mut read_for_action)?;
        let mut delta = AppliedPatchDelta::empty();

        for change in &action.changes {
            match change {
                PatchChange::Add { path, content, .. } => {
                    if Path::new(&path).exists() {
                        return Err(patch_apply_failure(
                            format!("Cannot add existing file {path}"),
                            &delta,
                        ));
                    }
                    if !dry_run {
                        write_text_file(path, content, &mut delta)?;
                        delta.push_change(AppliedPatchChange {
                            path: path.clone(),
                            change: AppliedPatchFileChange::Add {
                                content: content.clone(),
                                overwritten_content: None,
                            },
                        });
                        if let Some(read_state) = &self.read_state {
                            record_patch_write_best_effort(read_state, path, content);
                        }
                    }
                }
                PatchChange::Delete { path, content, .. } => {
                    if !Path::new(&path).exists() {
                        return Err(patch_apply_failure(
                            format!("Cannot delete missing file {path}"),
                            &delta,
                        ));
                    }
                    if !dry_run {
                        if let Some(read_state) = &self.read_state {
                            read_state.forget(path)?;
                        }
                        if let Err(err) = fs::remove_file(path) {
                            if !file_still_matches(path, content) {
                                delta.mark_inexact();
                            }
                            return Err(patch_apply_failure(
                                format!("Failed to delete file {path}: {err}"),
                                &delta,
                            ));
                        }
                        delta.push_change(AppliedPatchChange {
                            path: path.clone(),
                            change: AppliedPatchFileChange::Delete {
                                content: content.clone(),
                            },
                        });
                    }
                }
                PatchChange::Update {
                    path,
                    move_to,
                    original_content,
                    new_content,
                    ..
                } => {
                    if !dry_run {
                        let write_path = move_to.as_deref().unwrap_or(path);
                        if let Some(target) = &move_to
                            && target != path
                        {
                            let overwritten_move_content =
                                read_optional_existing_text(target, &mut delta);
                            write_text_file(target, new_content, &mut delta)?;
                            let target_delta_index = delta.push_change(AppliedPatchChange {
                                path: target.clone(),
                                change: AppliedPatchFileChange::Add {
                                    content: new_content.clone(),
                                    overwritten_content: overwritten_move_content.clone(),
                                },
                            });
                            if let Some(read_state) = &self.read_state {
                                read_state.forget(path)?;
                            }
                            if let Err(err) = fs::remove_file(path) {
                                if !file_still_matches(path, original_content) {
                                    delta.mark_inexact();
                                }
                                return Err(patch_apply_failure(
                                    format!("Failed to delete moved source file {path}: {err}"),
                                    &delta,
                                ));
                            }
                            delta.replace_change(
                                target_delta_index,
                                AppliedPatchChange {
                                    path: path.clone(),
                                    change: AppliedPatchFileChange::Update {
                                        move_to: Some(target.clone()),
                                        original_content: original_content.clone(),
                                        overwritten_move_content,
                                        new_content: new_content.clone(),
                                    },
                                },
                            );
                            if let Some(read_state) = &self.read_state {
                                record_patch_write_best_effort(read_state, target, new_content);
                            }
                            continue;
                        }
                        write_text_file(write_path, new_content, &mut delta)?;
                        delta.push_change(AppliedPatchChange {
                            path: path.clone(),
                            change: AppliedPatchFileChange::Update {
                                move_to: None,
                                original_content: original_content.clone(),
                                overwritten_move_content: None,
                                new_content: new_content.clone(),
                            },
                        });
                        if let Some(read_state) = &self.read_state {
                            record_patch_write_best_effort(read_state, write_path, new_content);
                        }
                    }
                }
            }
        }

        Ok(json!({
            "status": if dry_run { "validated" } else { "applied" },
            "files_changed": action.stats.files_changed,
            "hunks_applied": action.stats.hunks_applied,
            "created_files": action.stats.created_files,
            "deleted_files": action.stats.deleted_files,
            "moved_files": action.stats.moved_files.iter().map(|move_op| {
                json!({ "from": move_op.from, "to": move_op.to })
            }).collect::<Vec<_>>(),
            "updated_files": action.stats.updated_files,
            "line_delta": {
                "added": action.stats.added_lines,
                "removed": action.stats.removed_lines,
            },
            "summary": action.summary(),
            "diff_preview": action.preview.text,
            "diff_truncated": action.preview.truncated,
            "applied_delta": applied_delta_json(&delta),
        }))
    }
}

fn record_patch_write_best_effort(read_state: &FileReadState, path: &str, content: &str) {
    if let Err(err) = read_state.record_write(path, content) {
        eprintln!("Failed to record file read state after patch write: {err}");
    }
}

fn write_text_file(
    path: &str,
    content: &str,
    delta: &mut AppliedPatchDelta,
) -> Result<(), ToolError> {
    if let Some(parent) = Path::new(path).parent()
        && let Err(err) = fs::create_dir_all(parent)
    {
        return Err(patch_apply_failure(
            format!("Failed to create parent directory for {path}: {err}"),
            delta,
        ));
    }
    if let Err(err) = fs::write(path, content) {
        delta.mark_inexact();
        return Err(patch_apply_failure(
            format!("Failed to write file {path}: {err}"),
            delta,
        ));
    }
    Ok(())
}

fn patch_apply_failure(message: impl Into<String>, delta: &AppliedPatchDelta) -> ToolError {
    ToolError::ExecutionFailed(format!(
        "{}\napplied_delta: {}",
        message.into(),
        applied_delta_json(delta)
    ))
}

fn applied_delta_json(delta: &AppliedPatchDelta) -> Value {
    json!({
        "exact": delta.is_exact(),
        "changes": delta.changes().iter().map(applied_patch_change_json).collect::<Vec<_>>(),
    })
}

fn applied_patch_change_json(change: &AppliedPatchChange) -> Value {
    match &change.change {
        AppliedPatchFileChange::Add {
            content,
            overwritten_content,
        } => json!({
            "path": change.path,
            "kind": "add",
            "content": content,
            "overwritten_content": overwritten_content,
        }),
        AppliedPatchFileChange::Delete { content } => json!({
            "path": change.path,
            "kind": "delete",
            "content": content,
        }),
        AppliedPatchFileChange::Update {
            move_to,
            original_content,
            overwritten_move_content,
            new_content,
        } => json!({
            "path": change.path,
            "kind": "update",
            "move_to": move_to,
            "original_content": original_content,
            "overwritten_move_content": overwritten_move_content,
            "new_content": new_content,
        }),
    }
}

fn file_still_matches(path: &str, expected_content: &str) -> bool {
    read_file_content(path).is_ok_and(|content| content == expected_content)
}

fn read_optional_existing_text(path: &str, delta: &mut AppliedPatchDelta) -> Option<String> {
    match read_file_content(path) {
        Ok(content) => Some(content),
        Err(ToolError::Io(err)) if err.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            delta.mark_inexact();
            None
        }
    }
}

impl From<PatchError> for ToolError {
    fn from(error: PatchError) -> Self {
        match error {
            PatchError::InvalidInput(message) => Self::InvalidInput(message),
            PatchError::ExecutionFailed(message) => Self::ExecutionFailed(message),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::ApplyPatchTool;
    use crate::file::{FileReadState, ReadFileTool};
    use crate::tool::Tool;

    #[test]
    fn apply_patch_description_encodes_safe_edit_contract() {
        let tool = ApplyPatchTool::default();
        let description = tool.description();

        assert!(description.contains("structured file edits"));
        assert!(description.contains("related edits across multiple locations"));
        assert!(description.contains("instead of shell sed"));
        assert!(description.contains("heredocs"));
    }

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
        assert_eq!(result["applied_delta"]["exact"], true);
        assert_eq!(result["applied_delta"]["changes"][0]["kind"], "update");
        assert_eq!(
            result["applied_delta"]["changes"][0]["original_content"],
            "hello\nworld\n"
        );
        assert_eq!(
            result["applied_delta"]["changes"][0]["new_content"],
            "hi\nworld\n"
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
        assert_eq!(result["applied_delta"]["exact"], true);
        assert_eq!(
            result["applied_delta"]["changes"].as_array().unwrap().len(),
            0
        );
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
        assert_eq!(result["applied_delta"]["changes"][0]["kind"], "add");
        assert_eq!(
            result["applied_delta"]["changes"][0]["content"],
            "hello\nworld\n"
        );
    }

    #[tokio::test]
    async fn reports_applied_delta_after_partial_patch_failure() {
        let dir = tempfile::tempdir().expect("tempdir");
        let created = dir.path().join("created.txt");
        let parent_file = dir.path().join("not_a_directory");
        let blocked_child = parent_file.join("child.txt");
        std::fs::write(&parent_file, "parent\n").expect("write parent file");

        let tool = ApplyPatchTool::default();
        let error = tool
            .call(json!({
                "patch": format!(
                    "*** Begin Patch\n*** Add File: {}\n+created\n*** Add File: {}\n+blocked\n*** End Patch",
                    created.display(),
                    blocked_child.display()
                )
            }))
            .await
            .expect_err("second write should fail");
        let message = error.to_string();

        assert!(
            message.contains("Failed to create parent directory"),
            "unexpected error: {message}"
        );
        assert!(
            message.contains("\"exact\":true"),
            "delta should be exact when failure happens before the blocked write: {message}"
        );
        assert!(
            message.contains("\"kind\":\"add\""),
            "delta should include the committed add: {message}"
        );
        assert!(
            message.contains(&created.display().to_string()),
            "delta should include the committed path: {message}"
        );
        assert_eq!(
            std::fs::read_to_string(&created).expect("created file remains"),
            "created\n"
        );
    }

    #[tokio::test]
    async fn update_patch_allows_full_read_when_hunk_matches_current_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");
        let read_state = Arc::new(FileReadState::default());
        let read_tool = ReadFileTool::new(read_state.clone());
        let patch_tool = ApplyPatchTool::new(read_state.clone());

        // Full read (no offset/limit).
        read_tool
            .call(json!({
                "path": file.display().to_string()
            }))
            .await
            .expect("full read");
        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
            file.display()
        );
        patch_tool
            .call(json!({ "patch": patch }))
            .await
            .expect("patch after full read");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "hi\nworld\n");
    }

    #[tokio::test]
    async fn update_patch_rejects_stale_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "hello\nworld\n").expect("write");
        let read_state = Arc::new(FileReadState::default());
        let read_tool = ReadFileTool::new(read_state.clone());
        let patch_tool = ApplyPatchTool::new(read_state.clone());

        read_tool
            .call(json!({ "path": file.display().to_string() }))
            .await
            .expect("full read");

        // Modify file after read — simulates formatter or user edit.
        std::fs::write(&file, "goodbye\nworld\n").expect("external write");

        let patch = format!(
            "*** Begin Patch\n*** Update File: {}\n@@\n-hello\n+hi\n world\n*** End Patch",
            file.display()
        );
        let err = patch_tool
            .call(json!({ "patch": patch }))
            .await
            .expect_err("stale file should be rejected");
        assert!(
            err.to_string().contains("changed since read"),
            "unexpected stale-file error: {err}"
        );
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
    async fn update_patch_preserves_curly_quotes_in_new_text() {
        // When the file uses curly quotes and the model sends straight
        // quotes in both old_string and new_string, the written file
        // should keep the original curly-quote style.
        let dir = tempfile::tempdir().expect("tempdir");
        let file = dir.path().join("sample.txt");
        std::fs::write(&file, "\u{201C}old\u{201D}\nworld\n").expect("write");

        let tool = ApplyPatchTool::default();
        tool.call(json!({
            "patch": format!(
                "*** Begin Patch\n*** Update File: {}\n@@\n-\"old\"\n+\"new\"\n world\n*** End Patch",
                file.display()
            )
        }))
        .await
        .expect("apply patch with curly quote preservation");

        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "\u{201C}new\u{201D}\nworld\n"
        );
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
