use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::*;
use crate::runtime_control::{InputControlRequest, ShellApprovalDecision};
use crate::tui::RuntimeCommand;
use crate::tui::composer_text::{WrapConfig, wrapped_text};
use crate::tui::testing::TuiHarness;

fn approval_harness() -> TuiHarness {
    let (_temp, app) = pending_shell_app();
    let mut snapshot = (*app.snapshot).clone();
    let approval = snapshot.pending_interactions[0].approval.as_mut().unwrap();
    let command = format!(
        "printf '%s\\n' '{}'\ncat <<'EOF'\n{}\nEOF\nCOMMAND_END",
        "wide \u{4e2d}\u{6587} \u{1f680} ".repeat(35),
        (0..20)
            .map(|row| format!("payload-{row:02}\n"))
            .collect::<String>(),
    );
    approval.command = command.clone();
    approval.payload.command = Some(command);
    approval.payload.cwd = Some(format!("/{}DIRECTORY_END", "long-directory/".repeat(20)));
    let mut tui = TuiHarness::new(snapshot).expect("approval harness");
    tui.app_mut()
        .push_entry(MessageRole::User, "Run the prepared command.");
    tui
}

async fn press(tui: &mut TuiHarness, code: KeyCode) {
    assert!(
        !tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn every_command_and_directory_row_is_reachable_before_approval() {
    for (width, height) in [(120, 24), (80, 24), (60, 14), (60, 10), (40, 8)] {
        let mut tui = approval_harness();
        let approval = tui.app().snapshot.pending_interactions[0]
            .approval
            .as_ref()
            .unwrap();
        let source = format!(
            "{}\ncwd: {}",
            approval.command,
            approval.payload.cwd.as_ref().unwrap()
        );
        let expected = wrapped_text(
            &source,
            WrapConfig {
                width,
                initial_indent: "  ",
                subsequent_indent: "  ",
            },
        )
        .rows()
        .to_vec();
        let mut seen = HashSet::new();
        let mut previous = String::new();
        for _ in 0..256 {
            let screen = tui.screen_text(width, height);
            for label in [
                "[1] Allow once",
                "[2] Allow prefix",
                "[3] Allow session",
                "[4] Reject",
            ] {
                assert!(
                    screen.contains(label),
                    "missing action at {width}x{height}: {screen}"
                );
            }
            if screen == previous {
                break;
            }
            // TestBackend represents the continuation cell of a wide glyph as
            // a space; compare content without those synthetic cell gaps.
            seen.extend(
                screen
                    .lines()
                    .map(|line| line.split_whitespace().collect::<String>()),
            );
            previous = screen;
            press(&mut tui, KeyCode::PageDown).await;
            tui.expect_no_commands();
        }
        for row in expected {
            assert!(
                seen.contains(&row.split_whitespace().collect::<String>()),
                "unreachable row at {width}x{height}: {row:?}"
            );
        }
        press(&mut tui, KeyCode::Char('1')).await;
        tui.expect_command(RuntimeCommand::Input(
            InputControlRequest::AnswerShellApproval {
                decision: ShellApprovalDecision::Once,
            },
        ));
    }
}

#[tokio::test]
async fn detail_navigation_preserves_selection_and_clamps_after_resize() {
    let mut tui = approval_harness();
    tui.screen_text(60, 10);
    press(&mut tui, KeyCode::Down).await;
    let first = tui.screen_text(60, 10);
    let cwd_row = first
        .lines()
        .position(|line| line.trim_start().starts_with("cwd:"))
        .unwrap();
    press(&mut tui, KeyCode::PageDown).await;
    let second = tui.screen_text(60, 10);
    assert_ne!(first, second);
    assert!(
        second
            .lines()
            .nth(cwd_row)
            .unwrap()
            .trim_start()
            .starts_with("cwd:")
    );
    press(&mut tui, KeyCode::PageUp).await;
    assert_eq!(tui.screen_text(60, 10), first);
    press(&mut tui, KeyCode::End).await;
    assert!(tui.screen_text(60, 10).contains("DIRECTORY_END"));
    assert_eq!(tui.app().approval_picker_idx, 1);
    let expanded = tui.screen_text(120, 60);
    assert!(expanded.contains("printf"), "{expanded}");
    assert!(expanded.contains("DIRECTORY_END"), "{expanded}");
    tui.screen_text(40, 8);
    press(&mut tui, KeyCode::End).await;
    assert!(tui.screen_text(40, 8).contains("DIRECTORY_END"));
    press(&mut tui, KeyCode::Home).await;
    assert!(tui.screen_text(40, 8).contains("printf"));
    tui.expect_no_commands();
}

#[tokio::test]
async fn replacement_request_starts_at_its_first_row() {
    let mut tui = approval_harness();
    tui.screen_text(60, 10);
    press(&mut tui, KeyCode::End).await;
    assert!(tui.screen_text(60, 10).contains("DIRECTORY_END"));
    tui.app_mut().snapshot.pending_interactions[0]
        .approval
        .as_mut()
        .unwrap()
        .tool_use_id = "replacement".into();
    let screen = tui.screen_text(60, 10);
    assert!(screen.contains("printf"), "{screen}");
    tui.expect_no_commands();
}

#[tokio::test]
async fn restored_history_resets_details_even_when_the_call_id_is_reused() {
    let mut tui = approval_harness();
    tui.screen_text(60, 10);
    press(&mut tui, KeyCode::End).await;
    assert!(tui.screen_text(60, 10).contains("DIRECTORY_END"));
    tui.app_mut().restore_committed_turns(Vec::new());
    tui.app_mut()
        .push_entry(MessageRole::User, "Restored command.");
    let screen = tui.screen_text(60, 10);
    assert!(screen.contains("printf"), "{screen}");
    tui.expect_no_commands();
}

#[tokio::test]
async fn composer_and_overlay_keep_ownership_of_detail_navigation_keys() {
    let mut tui = approval_harness();
    let first = tui.screen_text(60, 10);
    tui.app_mut().bottom_pane.input = "draft".into();
    press(&mut tui, KeyCode::PageDown).await;
    press(&mut tui, KeyCode::End).await;
    assert_eq!(tui.app().bottom_pane.input, "draft");
    tui.app_mut().bottom_pane.input.clear();
    assert_eq!(tui.screen_text(60, 10), first);
    tui.app_mut()
        .open_overlay(Overlay::Help(crate::tui::state::HelpTab::General));
    press(&mut tui, KeyCode::End).await;
    press(&mut tui, KeyCode::PageDown).await;
    press(&mut tui, KeyCode::Esc).await;
    assert_eq!(tui.screen_text(60, 10), first);
    tui.expect_no_commands();
}

#[test]
fn transcript_retains_the_full_shell_approval_details() {
    let tui = approval_harness();
    let text = renderable_transcript_lines(tui.app(), 80)
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("COMMAND_END"), "{text}");
    assert!(text.contains("DIRECTORY_END"), "{text}");
}
