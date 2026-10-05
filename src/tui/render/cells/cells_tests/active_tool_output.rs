use super::*;
use crate::tui::message_role::MessageRole;

#[test]
fn active_turn_cell_hides_background_stdout_label_and_pins_stderr() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Run the formatter".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolProgress,
                message: [
                    "background task stdout:",
                    "info: downloading 6 components",
                    "background task stderr:",
                    "warning: retrying download",
                    "background task stdout:",
                    "info: installing rustfmt",
                ]
                .join("\n"),
                payload: None,
            },
        ],
    }
    .into();

    let rendered_lines = ActiveTurnCell::new(&app, Some(Path::new("."))).display_lines(100);
    let rendered = rendered_lines
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");

    assert!(!rendered.contains("background task stdout:"));
    assert!(!rendered.contains("background task stderr:"));
    assert!(rendered.contains("info: downloading 6 components"));
    assert!(rendered.contains("info: installing rustfmt"));
    assert!(rendered.contains("warning: retrying download"));

    let install_idx = rendered.find("info: installing rustfmt").unwrap();
    let stderr_idx = rendered.find("warning: retrying download").unwrap();
    assert!(install_idx < stderr_idx);

    let stderr_line = rendered_lines
        .iter()
        .find(|line| line.to_string().contains("warning: retrying download"))
        .expect("stderr line should render");
    assert!(
        stderr_line
            .spans
            .iter()
            .any(|span| { span.style.fg == Some(crate::tui::theme::TOOL_STDERR_FG) })
    );
}

#[test]
fn active_turn_cell_renders_terminal_result_as_terminal_cell() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Start the dev server".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "pty_start npm run dev".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: "pty pty-123 running: npm run dev\noutput:\nready\nlistening on 3000"
                    .into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Running pty npm run dev"));
    assert!(rendered.contains("└ ready"));
    assert!(rendered.contains("listening on 3000"));
    assert!(!rendered.contains("Run pty_start"));
}

#[test]
fn active_turn_cell_renders_latest_tool_result_diff_preview() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Edit the file".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::Tool,
                message: "replace src/main.rs".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: [
                    "replace src/main.rs",
                    "replacements=1 line_delta=0",
                    "diff:",
                    "*** Begin Patch",
                    "*** Update File: src/main.rs",
                    "@@",
                    "-old",
                    "+new",
                    "*** End Patch",
                ]
                .join("\n"),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Tool Result"));
    assert!(rendered.contains("replace src/main.rs"));
    assert!(rendered.contains("Edited src/main.rs"));
    assert!(rendered.contains("- old"));
    assert!(rendered.contains("+ new"));
}

#[test]
fn active_turn_cell_renders_typed_terminal_event_as_terminal_cell() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Start the dev server".into(),
                payload: None,
            },
            TranscriptEntry::terminal_event(TerminalEvent::End(TerminalCommandEvent {
                target: TerminalTarget::Pty,
                id: Some("pty-123".into()),
                status: "running".into(),
                command: Some("npm run dev".into()),
                exit_code: None,
                output: vec!["ready".into(), "listening on 3000".into()],
                output_path: None,
                is_error: false,
            })),
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Running pty npm run dev"));
    assert!(rendered.contains("└ ready"));
    assert!(rendered.contains("listening on 3000"));
    assert!(!rendered.contains("Terminal Event"));
}

#[test]
fn active_turn_cell_prefers_responding_over_tool_result_while_processing_response() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::ProcessingResponse;
    app.runtime_phase_detail = Some("waiting for model output".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Review the repository".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: "bash stdout: partial output".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• waiting for model output"));
    assert!(!rendered.contains("Tool Result"));
    assert!(!rendered.contains("bash stdout: partial output"));
}

#[test]
fn active_turn_cell_renders_bash_completion_as_status_line() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("tool completed".into()).into();
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Run checks".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: "bash finished with exit code 0\nDuration: 12 ms".into(),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("✓ bash"));
    assert!(rendered.contains("Duration: 12 ms"));
    assert!(!rendered.contains("Tool Result"));
    assert!(!rendered.contains("bash finished with exit code 0"));
}

#[test]
fn active_turn_cell_marks_truncated_bash_completion_body() {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .expect("build tui app");
    app.runtime_phase = RuntimePhase::RunningTool;
    app.runtime_phase_detail = Some("tool completed".into()).into();
    let body = (1..=20)
        .map(|idx| format!("line {idx}"))
        .collect::<Vec<_>>()
        .join("\n");
    app.active_turn = TranscriptTurn {
        thinking_duration: None,
        entries: vec![
            TranscriptEntry {
                role: MessageRole::User,
                message: "Run checks".into(),
                payload: None,
            },
            TranscriptEntry {
                role: MessageRole::ToolResult,
                message: format!("bash finished with exit code 0\n{body}"),
                payload: None,
            },
        ],
    }
    .into();

    let rendered = ActiveTurnCell::new(&app, Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("✓ bash"));
    assert!(rendered.contains("line 1"));
    assert!(rendered.contains("... 8 more line(s)"));
    assert!(!rendered.contains("line 20"));
}
