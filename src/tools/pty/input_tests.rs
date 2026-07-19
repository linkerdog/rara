use serde_json::json;

use super::types::PtyCommandInput;

#[test]
fn parses_structured_program_payload() {
    let input = PtyCommandInput::from_value(json!({
        "program": "cargo",
        "args": ["check", "--workspace"],
        "cwd": "/tmp",
        "allow_net": true,
        "rows": 30,
        "cols": 100,
    }))
    .expect("structured pty payload");

    assert_eq!(input.program.as_deref(), Some("cargo"));
    assert_eq!(
        input.args,
        vec!["check".to_string(), "--workspace".to_string()]
    );
    assert_eq!(input.cwd.as_deref(), Some("/tmp"));
    assert!(input.allow_net);
    assert_eq!(input.rows, 30);
    assert_eq!(input.cols, 100);
    assert_eq!(input.summary(), "cargo check --workspace");
}

#[test]
fn parses_legacy_command_payload() {
    let input = PtyCommandInput::from_value(json!({
        "command": "echo hello",
    }))
    .expect("legacy pty payload");

    assert_eq!(input.command.as_deref(), Some("echo hello"));
    assert_eq!(input.summary(), "echo hello");
    assert_eq!(input.rows, 24);
    assert_eq!(input.cols, 120);
}

#[test]
fn rejects_missing_command_and_program() {
    let err = PtyCommandInput::from_value(json!({
        "rows": 24,
        "cols": 120,
    }))
    .expect_err("no command or program");

    assert!(
        err.to_string().contains("either command or program"),
        "{err}"
    );
}

#[test]
fn rejects_zero_rows() {
    let err = PtyCommandInput::from_value(json!({
        "command": "echo hi",
        "rows": 0,
    }))
    .expect_err("zero rows");

    assert!(err.to_string().contains("rows must be >= 1"), "{err}");
}

#[test]
fn rejects_zero_cols() {
    let err = PtyCommandInput::from_value(json!({
        "command": "echo hi",
        "cols": 0,
    }))
    .expect_err("zero cols");

    assert!(err.to_string().contains("cols must be >= 1"), "{err}");
}

#[test]
fn whitespace_command_falls_back_to_program() {
    let input = PtyCommandInput::from_value(json!({
        "command": "   ",
        "program": "cargo",
        "args": ["test"],
    }))
    .expect("whitespace command");

    assert_eq!(input.program.as_deref(), Some("cargo"));
    assert_eq!(input.summary(), "cargo test");
}
