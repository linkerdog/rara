use crossterm::event::{Event, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};

use super::*;
use crate::tui::state::Overlay;
use crate::tui::testing::TuiHarness;

fn harness() -> TuiHarness {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().bottom_pane.input = "hidden composer draft".into();
    tui.app_mut()
        .open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
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
async fn resume_query_owns_grapheme_editing_paste_and_escape() {
    let mut tui = harness();
    tui.send_terminal_event(Event::Paste(
        "a\u{754c}e\u{301}\u{1f469}\u{200d}\u{1f4bb}z".into(),
    ))
    .await
    .unwrap();
    press(&mut tui, KeyCode::Left).await;
    press(&mut tui, KeyCode::Backspace).await;
    assert_eq!(tui.app().resume_search_query, "a\u{754c}e\u{301}z");
    press(&mut tui, KeyCode::Home).await;
    press(&mut tui, KeyCode::Right).await;
    press(&mut tui, KeyCode::Delete).await;
    assert_eq!(tui.app().resume_search_query, "ae\u{301}z");
    tui.press_key(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(tui.app().resume_search_query, "az");
    press(&mut tui, KeyCode::Char('b')).await;
    press(&mut tui, KeyCode::End).await;
    tui.send_terminal_event(Event::Paste("!".into()))
        .await
        .unwrap();
    assert_eq!(tui.app().resume_search_query, "abz!");
    assert_eq!(tui.app().bottom_pane.input, "hidden composer draft");
    press(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().resume_search_query.is_empty());
    assert!(tui.app().overlay.is_some());
    press(&mut tui, KeyCode::Esc).await;
    assert_eq!(tui.app().overlay, None);
    assert_eq!(tui.app().bottom_pane.input, "hidden composer draft");
    tui.expect_no_commands();
}

#[tokio::test]
async fn resume_footer_controls_scope_sort_refresh_and_selection() {
    let mut tui = harness();
    press(&mut tui, KeyCode::Tab).await;
    assert_eq!(tui.app().resume_query.scope_label(), "all");
    press(&mut tui, KeyCode::BackTab).await;
    assert_eq!(tui.app().resume_query.scope_label(), "cwd");
    let before = tui.app().resume_sort_by_created;
    tui.press_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(tui.app().resume_sort_by_created, !before);
    tui.app_mut().resume_query.error = Some("scripted query failure".into());
    assert!(tui.screen_text(100, 30).contains("scripted query failure"));
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert!(tui.app().resume_query.error.is_none());
    assert!(tui.app().resume_search_query.is_empty());

    tui.app_mut().recent_threads = (0..30)
        .map(|i| super::tests::thread_summary(&format!("saved-{i}"), "/b/app"))
        .collect();
    let screen = tui.screen_text(100, 30);
    assert!(screen.contains("cwd=/b/app"), "{screen}");
    let page = tui.app().resume_query.page_items;
    assert!(page > 1);
    press(&mut tui, KeyCode::PageDown).await;
    assert_eq!(tui.app().resume_picker_idx, page);
    press(&mut tui, KeyCode::PageUp).await;
    assert_eq!(tui.app().resume_picker_idx, 0);
    press(&mut tui, KeyCode::Down).await;
    assert_eq!(tui.app().resume_picker_idx, 1);
    press(&mut tui, KeyCode::Up).await;
    assert_eq!(tui.app().resume_picker_idx, 0);
    tui.send_terminal_event(Event::Mouse(MouseEvent {
        kind: MouseEventKind::ScrollDown,
        column: 10,
        row: 10,
        modifiers: KeyModifiers::NONE,
    }))
    .await
    .unwrap();
    assert!(tui.app().resume_picker_idx > 0);
    assert_eq!(
        selected_resumable_thread_id(tui.app()),
        Some(format!("saved-{}", tui.app().resume_picker_idx))
    );
    assert!(matches!(
        crate::tui::keymap::map_key_to_event(
            KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE),
            tui.app()
        ),
        AppEvent::ApplyOverlaySelection
    ));
    assert_eq!(tui.app().bottom_pane.input, "hidden composer draft");
}

#[tokio::test]
async fn resume_cursor_and_footer_remain_visible_at_narrow_widths() {
    let mut tui = harness();
    tui.send_terminal_event(Event::Paste(format!(
        "{}tail",
        "\u{754c}e\u{301}".repeat(50)
    )))
    .await
    .unwrap();
    for width in [40, 60, 80] {
        let (screen, cursor) = tui.screen_with_cursor(width, 24);
        let (x, y) = cursor.expect("resume query cursor");
        assert!(x < width && y < 24);
        let query_line = screen.lines().nth(y as usize).unwrap();
        assert!(query_line.contains("Search:"), "{screen}");
        assert!(query_line.contains("tail"), "{screen}");
        for hint in [
            "Tab cwd/all",
            "Ctrl+S sort",
            "Ctrl+R retry",
            "PgUp/PgDn page",
            "Enter resume",
            "Esc clear/close",
        ] {
            assert!(screen.contains(hint), "missing {hint}:\n{screen}");
        }
        press(&mut tui, KeyCode::Home).await;
        let (home, home_cursor) = tui.screen_buffer(width, 24);
        let (home_x, home_y) = home_cursor.unwrap();
        assert_eq!(home[(home_x, home_y)].symbol(), "\u{754c}");
        assert_eq!(home[(home_x + 2, home_y)].symbol(), "e\u{301}");
        assert!(home_x < x);
        press(&mut tui, KeyCode::End).await;
    }
}
