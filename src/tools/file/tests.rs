use std::sync::Arc;

use serde_json::{Value, json};

use super::{
    FileReadState, ListFilesTool, MAX_READ_LINE_BYTES, MAX_READ_LINE_CHARS, ReadFileTool,
    ReplaceLinesTool, ReplaceTool, WriteFileTool,
};
use crate::tool::Tool;

#[tokio::test]
async fn read_file_supports_line_ranges() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "a\nb\nc\nd\n").expect("write sample");

    let tool = ReadFileTool::default();
    let result = tool
        .call(json!({
            "path": path.display().to_string(),
            "start_line": 2,
            "end_line": 3
        }))
        .await
        .expect("read file");

    assert_eq!(result["content"], "b\nc");
    assert_eq!(result["total_lines"], Value::Null);
    assert_eq!(result["total_lines_exact"], false);
    assert_eq!(result["observed_lines"], 4);
    assert_eq!(result["start_line"], 2);
    assert_eq!(result["end_line"], 3);
    assert_eq!(result["offset"], 2);
    assert_eq!(result["limit"], 2);
    assert_eq!(result["num_lines"], 2);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["next_offset"], 4);
}

#[tokio::test]
async fn read_file_supports_offset_limit_windows() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "alpha\nbeta\ngamma\n").expect("write sample");

    let tool = ReadFileTool::default();
    let result = tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 2,
            "limit": 1
        }))
        .await
        .expect("read file");

    assert_eq!(result["content"], "beta");
    assert_eq!(result["total_lines"], Value::Null);
    assert_eq!(result["total_lines_exact"], false);
    assert_eq!(result["observed_lines"], 3);
    assert_eq!(result["start_line"], 2);
    assert_eq!(result["end_line"], 2);
    assert_eq!(result["num_lines"], 1);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["next_offset"], 3);
}

#[tokio::test]
async fn read_file_defaults_to_limited_first_window() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("large.txt");
    let content = (1..=2002)
        .map(|line| format!("line {line}"))
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&path, content).expect("write sample");

    let tool = ReadFileTool::default();
    let result = tool
        .call(json!({ "path": path.display().to_string() }))
        .await
        .expect("read file");

    assert!(result["content"].as_str().unwrap().contains("line 1"));
    assert!(result["content"].as_str().unwrap().contains("line 2000"));
    assert!(!result["content"].as_str().unwrap().contains("line 2001"));
    assert_eq!(result["total_lines"], Value::Null);
    assert_eq!(result["total_lines_exact"], false);
    assert_eq!(result["observed_lines"], 2001);
    assert_eq!(result["start_line"], 1);
    assert_eq!(result["end_line"], 2000);
    assert_eq!(result["limit"], 2000);
    assert_eq!(result["num_lines"], 2000);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["next_offset"], 2001);
}

#[tokio::test]
async fn read_file_errors_when_offset_exceeds_file_length() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\n").expect("write sample");

    let tool = ReadFileTool::default();
    let error = tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 3
        }))
        .await
        .expect_err("offset should fail");

    assert!(error.to_string().contains("offset 3 exceeds file length 2"));
}

#[tokio::test]
async fn read_file_marks_truncated_long_lines() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "x".repeat(MAX_READ_LINE_CHARS + 1)).expect("write sample");

    let tool = ReadFileTool::default();
    let result = tool
        .call(json!({ "path": path.display().to_string() }))
        .await
        .expect("read file");

    assert_eq!(result["total_lines"], 1);
    assert_eq!(result["total_lines_exact"], true);
    assert_eq!(result["line_truncated"], true);
    assert_eq!(result["truncated"], true);
    assert!(
        result["content"]
            .as_str()
            .unwrap()
            .ends_with("... [line truncated]")
    );
}

#[tokio::test]
async fn read_file_bounds_memory_for_very_long_lines() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    let content = format!("{}\nsecond\n", "x".repeat(MAX_READ_LINE_BYTES * 2));
    std::fs::write(&path, content).expect("write sample");

    let tool = ReadFileTool::default();
    let result = tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 1,
            "limit": 1
        }))
        .await
        .expect("read file");

    let content = result["content"].as_str().expect("content");
    assert!(content.ends_with("... [line truncated]"));
    assert!(content.chars().count() < MAX_READ_LINE_BYTES);
    assert_eq!(result["line_truncated"], true);
    assert_eq!(result["truncated"], true);
    assert_eq!(result["next_offset"], 2);
}

#[tokio::test]
async fn list_files_skips_build_artifacts_by_default() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path();
    std::fs::create_dir_all(root.join("src")).expect("mkdir src");
    std::fs::create_dir_all(root.join("target/debug")).expect("mkdir target");
    std::fs::write(root.join("src/main.rs"), "fn main() {}\n").expect("write source");
    std::fs::write(root.join("target/debug/app"), "bin").expect("write artifact");

    let tool = ListFilesTool;
    let result = tool
        .call(json!({ "path": root.display().to_string() }))
        .await
        .expect("list files");
    let files = result["files"].as_array().expect("files array");
    let rendered = files
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("src/main.rs"));
    assert!(!rendered.contains("target/debug/app"));
}

#[tokio::test]
async fn list_files_can_include_build_artifacts_when_requested() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path();
    std::fs::create_dir_all(root.join("target/debug")).expect("mkdir target");
    std::fs::write(root.join("target/debug/app"), "bin").expect("write artifact");

    let tool = ListFilesTool;
    let result = tool
        .call(json!({
            "path": root.display().to_string(),
            "include_ignored": true
        }))
        .await
        .expect("list files");
    let files = result["files"].as_array().expect("files array");
    let rendered = files
        .iter()
        .filter_map(|value| value.as_str())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("target/debug/app"));
}

#[tokio::test]
async fn list_files_reports_limit_and_truncated_status() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let root = tempdir.path();
    std::fs::write(root.join("a.txt"), "a").expect("write a");
    std::fs::write(root.join("b.txt"), "b").expect("write b");

    let tool = ListFilesTool;
    let result = tool
        .call(json!({
            "path": root.display().to_string(),
            "limit": 1
        }))
        .await
        .expect("list files");

    assert_eq!(result["files"].as_array().expect("files array").len(), 1);
    assert_eq!(result["total_count"], 2);
    assert_eq!(result["truncated"], true);
}

#[tokio::test]
async fn write_file_reports_created_or_overwritten() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");

    let tool = WriteFileTool::default();
    let created = tool
        .call(json!({
            "path": path.display().to_string(),
            "content": "hello\nworld\n"
        }))
        .await
        .expect("write created");
    assert_eq!(created["operation"], "created");
    assert_eq!(created["line_count"], 2);

    let overwritten = tool
        .call(json!({
            "path": path.display().to_string(),
            "content": "updated\n"
        }))
        .await
        .expect("write overwritten");
    assert_eq!(overwritten["operation"], "overwritten");
    assert_eq!(overwritten["previous_line_count"], 2);
}

#[tokio::test]
async fn replace_reports_preview_and_line_delta() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "hello old value\n").expect("write sample");

    let tool = ReplaceTool::default();
    let result = tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "old value",
            "new_string": "new\nvalue"
        }))
        .await
        .expect("replace content");

    assert_eq!(result["replacements"], 1);
    assert_eq!(result["old_preview"], "old value");
    assert_eq!(result["new_preview"], "new\\nvalue");
    assert_eq!(result["line_delta"], 1);
}

#[tokio::test]
async fn replace_requires_prior_full_read_when_state_is_enabled() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "hello old value\n").expect("write sample");
    let read_state = Arc::new(FileReadState::default());
    let replace_tool = ReplaceTool::new(read_state.clone());

    let error = replace_tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "old value",
            "new_string": "new value"
        }))
        .await
        .expect_err("replace should require read");
    assert!(error.to_string().contains("File has not been read yet"));

    let read_tool = ReadFileTool::new(read_state);
    read_tool
        .call(json!({ "path": path.display().to_string() }))
        .await
        .expect("read file");
    replace_tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "old value",
            "new_string": "new value"
        }))
        .await
        .expect("replace after read");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read updated"),
        "hello new value\n"
    );
}

#[tokio::test]
async fn replace_allows_partial_read_when_old_string_is_unique() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("write sample");
    let read_state = Arc::new(FileReadState::default());
    let read_tool = ReadFileTool::new(read_state.clone());
    let replace_tool = ReplaceTool::new(read_state);

    read_tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 1,
            "limit": 1
        }))
        .await
        .expect("partial read");
    replace_tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "two",
            "new_string": "second"
        }))
        .await
        .expect("replace after partial read");

    assert_eq!(
        std::fs::read_to_string(&path).expect("read updated"),
        "one\nsecond\nthree\n"
    );
}

#[tokio::test]
async fn replace_lines_allows_offset_read_when_lines_not_truncated() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("write sample");
    let read_state = Arc::new(FileReadState::default());
    let read_tool = ReadFileTool::new(read_state.clone());
    let replace_lines_tool = ReplaceLinesTool::new(read_state);

    // Read only first line — not truncated, just a sub-range.
    read_tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 1,
            "limit": 1
        }))
        .await
        .expect("offset read");
    // Claude Code / Codex semantics: sub-range reads are not partial.
    // replace_lines re-reads the file internally, so line-number
    // validation is still correct.
    let result = replace_lines_tool
        .call(json!({
            "path": path.display().to_string(),
            "start_line": 2,
            "end_line": 2,
            "new_string": "second"
        }))
        .await
        .expect("replace_lines after offset read");
    assert!(result["path"].as_str().unwrap().ends_with("sample.txt"));
    let updated = std::fs::read_to_string(&path).expect("read file");
    assert_eq!(updated, "one\nsecond\nthree\n");
}

#[tokio::test]
async fn partial_read_does_not_downgrade_existing_full_read_state() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\n").expect("write sample");
    let read_state = Arc::new(FileReadState::default());
    let read_tool = ReadFileTool::new(read_state.clone());
    let replace_tool = ReplaceTool::new(read_state);

    read_tool
        .call(json!({ "path": path.display().to_string() }))
        .await
        .expect("full read");
    read_tool
        .call(json!({
            "path": path.display().to_string(),
            "offset": 2,
            "limit": 1
        }))
        .await
        .expect("partial read");

    replace_tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "two",
            "new_string": "second"
        }))
        .await
        .expect("replace after full read and later partial read");
    assert_eq!(
        std::fs::read_to_string(&path).expect("read updated"),
        "one\nsecond\nthree\n"
    );
}

#[tokio::test]
async fn replace_rejects_file_changed_since_read() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\n").expect("write sample");
    let read_state = Arc::new(FileReadState::default());
    let read_tool = ReadFileTool::new(read_state.clone());
    let replace_tool = ReplaceTool::new(read_state);

    read_tool
        .call(json!({ "path": path.display().to_string() }))
        .await
        .expect("read file");
    std::fs::write(&path, "one\nexternal\n").expect("external write");

    let error = replace_tool
        .call(json!({
            "path": path.display().to_string(),
            "old_string": "external",
            "new_string": "changed"
        }))
        .await
        .expect_err("stale file should be rejected");
    assert!(error.to_string().contains("since read"));
}

#[cfg(unix)]
#[tokio::test]
async fn forget_removes_canonical_read_state_for_symlink_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    let link = tempdir.path().join("linked.txt");
    std::fs::write(&path, "one\ntwo\n").expect("write sample");
    std::os::unix::fs::symlink(&path, &link).expect("create symlink");

    let read_state = Arc::new(FileReadState::default());
    let read_tool = ReadFileTool::new(read_state.clone());

    read_tool
        .call(json!({ "path": link.display().to_string() }))
        .await
        .expect("read file through symlink");
    read_state
        .forget(&link.display().to_string())
        .expect("forget symlink path");

    let error = read_state
        .validate_existing_edit(&path.display().to_string())
        .expect_err("forget should remove the canonical entry");
    assert!(error.to_string().contains("File has not been read yet"));
}

#[tokio::test]
async fn replace_lines_replaces_inclusive_range_without_old_string() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let path = tempdir.path().join("sample.txt");
    std::fs::write(&path, "one\ntwo\nthree\nfour\n").expect("write sample");

    let tool = ReplaceLinesTool::default();
    let result = tool
        .call(json!({
            "path": path.display().to_string(),
            "start_line": 2,
            "end_line": 3,
            "new_string": "middle"
        }))
        .await
        .expect("replace lines");

    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        "one\nmiddle\nfour\n"
    );
    assert_eq!(result["removed_lines"], 2);
    assert_eq!(result["removed_string"], "two\nthree");
    assert_eq!(result["inserted_lines"], 1);
    assert_eq!(result["line_delta"], -1);
}

#[test]
fn file_tool_descriptions_encode_safe_edit_contract() {
    let write_tool = WriteFileTool::default();
    let replace_tool = ReplaceTool::default();
    let replace_lines_tool = ReplaceLinesTool::default();

    assert!(write_tool.description().contains("Create a new file"));
    assert!(
        write_tool
            .description()
            .contains("read the full file first")
    );
    assert!(write_tool.description().contains("large write fails"));
    assert!(write_tool.description().contains("shell heredocs"));
    assert!(
        write_tool
            .input_schema()
            .to_string()
            .contains("shell heredoc fallbacks")
    );
    assert!(replace_tool.description().contains("exact, unique string"));
    assert!(
        replace_tool
            .description()
            .contains("Read the relevant file content first")
    );
    assert!(
        replace_lines_tool
            .description()
            .contains("verifying the current line numbers")
    );
}
