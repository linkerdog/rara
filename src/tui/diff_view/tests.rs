use std::process::Command;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use super::*;
use crate::tui::testing::TuiHarness;

async fn key(tui: &mut TuiHarness, code: KeyCode) {
    tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
        .await
        .unwrap();
}

async fn finish_capture(tui: &mut TuiHarness) {
    tokio::time::timeout(Duration::from_secs(10), async {
        while tui.app().diff_view.pending.is_some() {
            poll(tui.app_mut()).await;
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn production_diff_command_distinguishes_clean_tree_git_failure_and_changes() {
    let dir = tempfile::tempdir().unwrap();
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().snapshot.cwd = dir.path().display().to_string();
    for expected in [
        "Could not collect changes",
        "No staged or unstaged changes",
        "+new staged line",
    ] {
        tui.app_mut().bottom_pane.input = "/diff".into();
        tui.app_mut().sync_command_palette_with_input();
        key(&mut tui, KeyCode::Enter).await;
        assert_eq!(tui.app().overlay, Some(Overlay::Diff));
        finish_capture(&mut tui).await;
        assert!(
            tui.app().diff_view.text.contains(expected),
            "{}",
            tui.app().diff_view.text
        );
        assert!(tui.screen_text(80, 24).contains("Working tree diff"));
        key(&mut tui, KeyCode::Esc).await;
        if expected == "Could not collect changes" {
            assert!(
                Command::new("git")
                    .args(["init", "--quiet"])
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            );
        } else if expected == "No staged or unstaged changes" {
            std::fs::write(dir.path().join("test.txt"), "new staged line\n").unwrap();
            assert!(
                Command::new("git")
                    .args(["add", "--", "test.txt"])
                    .current_dir(dir.path())
                    .status()
                    .unwrap()
                    .success()
            );
            std::fs::write(
                dir.path().join("test.txt"),
                "new staged line\nnew unstaged line\n",
            )
            .unwrap();
        } else {
            assert!(tui.app().diff_view.text.contains("+new unstaged line"));
        }
    }
}

#[tokio::test]
async fn diff_scrolls_wrapped_rows_clamps_at_end_and_preserves_the_draft() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().bottom_pane.input = "keep this draft".into();
    tui.app_mut().diff_view.text = (0..100)
        .map(|i| format!("+row {i}: a long line with Unicode text: café and a complete tail\n"))
        .collect();
    tui.app_mut().open_overlay(Overlay::Diff);
    let screen = tui.screen_text(80, 24);
    assert!(screen.contains("+row 0:"));
    insta::assert_snapshot!("working_tree_diff", screen);
    key(&mut tui, KeyCode::End).await;
    let screen = tui.screen_text(40, 12);
    // Resizing changes wrapping; End must use the newly measured geometry.
    key(&mut tui, KeyCode::End).await;
    assert!(tui.screen_text(40, 12).contains("complete tail"));
    assert!(!screen.is_empty());
    let end = tui.app().diff_view.offset.get();
    for _ in 0..100 {
        key(&mut tui, KeyCode::Down).await;
    }
    assert_eq!(tui.app().diff_view.offset.get(), end);
    key(&mut tui, KeyCode::Up).await;
    assert_eq!(tui.app().diff_view.offset.get(), end - 1);
    key(&mut tui, KeyCode::Home).await;
    assert_eq!(tui.app().diff_view.offset.get(), 0);
    key(&mut tui, KeyCode::PageDown).await;
    assert_eq!(
        tui.app().diff_view.offset.get(),
        tui.app().diff_view.height.get()
    );
    tui.send_terminal_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: 10,
        row: 5,
        modifiers: KeyModifiers::NONE,
    }))
    .await
    .unwrap();
    assert!(tui.app().diff_view.offset.get() < tui.app().diff_view.height.get());
    key(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().overlay.is_none());
    assert_eq!(tui.app().bottom_pane.input, "keep this draft");
}

#[tokio::test]
async fn closing_diff_aborts_pending_capture_and_late_results_cannot_reopen_it() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let (sender, receiver) = tokio::sync::oneshot::channel::<String>();
    tui.app_mut().diff_view.pending = Some(tokio::spawn(async move { Ok(receiver.await?) }));
    tui.app_mut().open_overlay(Overlay::Diff);
    key(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().diff_view.pending.is_none());
    drop(sender);
    assert!(!poll(tui.app_mut()).await);
    assert!(tui.app().overlay.is_none());
}
