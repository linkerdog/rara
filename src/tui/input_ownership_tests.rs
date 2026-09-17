use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{ApiKeyTarget, HelpTab, ListPickerKind, Overlay, RuntimeSnapshot, StatusTab};
use super::terminal_ui::handle_paste;
use super::testing::TuiHarness;

fn harness() -> TuiHarness {
    TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness")
}

async fn press(tui: &mut TuiHarness, code: KeyCode) {
    assert!(
        !tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .expect("dispatch key")
    );
}

async fn type_text(tui: &mut TuiHarness, text: &str) {
    for ch in text.chars() {
        press(tui, KeyCode::Char(ch)).await;
    }
}

// INPUT-02: ordinary text must work from the first character.
#[tokio::test]
async fn ordinary_composer_accepts_initial_j_and_k() {
    for initial in ["just inspect", "keep working"] {
        let mut tui = harness();
        type_text(&mut tui, initial).await;
        assert_eq!(tui.app().bottom_pane.input, initial);
        assert!(tui.screen_text(100, 30).contains(initial));
        tui.expect_no_commands();
    }
}

// INPUT-04: editing the visible query must never edit the hidden composer.
#[tokio::test]
async fn model_search_owns_unicode_cursor_editing() {
    let mut tui = harness();
    tui.app_mut().bottom_pane.input = "draft".into();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(2);
    tui.app_mut().transcript_scroll = 7;
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, "模型ab").await;
    press(&mut tui, KeyCode::Home).await;
    press(&mut tui, KeyCode::Right).await;
    press(&mut tui, KeyCode::Char('X')).await;
    assert_eq!(tui.app().model_search_query, "模X型ab");
    press(&mut tui, KeyCode::Delete).await;
    assert_eq!(tui.app().model_search_query, "模Xab");
    press(&mut tui, KeyCode::Backspace).await;
    assert_eq!(tui.app().model_search_query, "模ab");
    press(&mut tui, KeyCode::End).await;
    press(&mut tui, KeyCode::Left).await;
    press(&mut tui, KeyCode::Char('y')).await;
    assert_eq!(tui.app().model_search_query, "模ayb");
    assert_eq!(tui.app().bottom_pane.input, "draft");
    assert_eq!(tui.app().bottom_pane.input_cursor_offset, Some(2));
    assert_eq!(tui.app().transcript_scroll, 7);
    tui.expect_no_commands();
}

#[tokio::test]
async fn deleting_model_query_resets_result_selection() {
    let mut tui = harness();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, "x").await;
    tui.app_mut().model_search_idx = 4;
    press(&mut tui, KeyCode::Backspace).await;
    assert!(tui.app().model_search_query.is_empty());
    assert_eq!(tui.app().model_search_idx, 0);
}

#[tokio::test]
async fn nested_search_dismissal_preserves_composer_draft_and_pastes() {
    let mut tui = harness();
    handle_paste("x".repeat(1200), tui.app_mut());
    tui.app_mut().bottom_pane.flush_paste_burst();
    let draft = tui.app().bottom_pane.input.clone();
    let cursor = tui.app().bottom_pane.input_cursor_offset;
    let pastes = tui.app().bottom_pane.large_paste_pending.clone();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, "query").await;
    press(&mut tui, KeyCode::Home).await;
    tui.app_mut().open_overlay(Overlay::Help(HelpTab::General));
    press(&mut tui, KeyCode::Esc).await;
    assert_eq!(tui.app().overlay, Some(Overlay::ModelSearch));
    press(&mut tui, KeyCode::Char('X')).await;
    assert_eq!(tui.app().model_search_query, "Xquery");
    press(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().overlay.is_none());
    assert!(tui.app().model_search_query.is_empty());
    assert_eq!(tui.app().bottom_pane.input, draft);
    assert_eq!(tui.app().bottom_pane.input_cursor_offset, cursor);
    assert_eq!(tui.app().bottom_pane.large_paste_pending, pastes);
    tui.expect_no_commands();
}

#[tokio::test]
async fn large_multiline_paste_belongs_to_model_search() {
    let mut tui = harness();
    tui.app_mut().bottom_pane.input = "draft".into();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    let paste = format!("head\r\n{}\rtail", "x".repeat(1200));
    handle_paste(paste, tui.app_mut());
    tui.app_mut().bottom_pane.flush_paste_burst();
    assert_eq!(
        tui.app().model_search_query,
        format!("head {} tail", "x".repeat(1200))
    );
    assert_eq!(tui.app().bottom_pane.input, "draft");
    assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
    tui.expect_no_commands();
}

#[tokio::test]
async fn setup_field_paste_and_cancel_preserve_underlying_draft() {
    let mut tui = harness();
    tui.app_mut().bottom_pane.input = "draft".into();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(1);
    tui.app_mut()
        .open_overlay(Overlay::ApiKeyEditor(ApiKeyTarget::DeepSeek));
    let key = "k".repeat(1200);
    handle_paste(key.clone(), tui.app_mut());
    tui.app_mut().bottom_pane.flush_paste_burst();
    assert_eq!(tui.app().api_key_input, key);
    press(&mut tui, KeyCode::Esc).await;
    assert_eq!(tui.app().bottom_pane.input, "draft");
    assert_eq!(tui.app().bottom_pane.input_cursor_offset, Some(1));
    tui.expect_no_commands();
}

#[test]
fn paste_does_not_modify_composer_behind_non_text_overlays() {
    for overlay in [
        Overlay::Help(HelpTab::General),
        Overlay::Status(StatusTab::Overview),
        Overlay::Context,
        Overlay::SkillsPicker,
        Overlay::PermissionPicker,
        Overlay::ListPicker(ListPickerKind::Provider),
    ] {
        let mut tui = harness();
        tui.app_mut().bottom_pane.input = "draft".into();
        tui.app_mut().open_overlay(overlay);
        handle_paste("injected\ntext".into(), tui.app_mut());
        tui.app_mut().bottom_pane.flush_paste_burst();
        assert_eq!(tui.app().bottom_pane.input, "draft", "{overlay:?}");
        assert!(tui.app().bottom_pane.large_paste_pending.is_empty());
        tui.expect_no_commands();
    }
}

#[test]
fn paste_in_resume_picker_updates_search_only() {
    let mut tui = harness();
    tui.app_mut().bottom_pane.input = "draft".into();
    tui.app_mut()
        .open_overlay(Overlay::ListPicker(ListPickerKind::Resume));
    handle_paste("old\r\nthread".into(), tui.app_mut());
    tui.app_mut().bottom_pane.flush_paste_burst();
    assert_eq!(tui.app().resume_search_query, "old thread");
    assert_eq!(tui.app().bottom_pane.input, "draft");
}

#[tokio::test]
async fn model_search_render_keeps_cursor_on_visible_query() {
    let mut tui = harness();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, "模型ab").await;
    press(&mut tui, KeyCode::Left).await;
    let (buffer, cursor) = tui.screen_buffer(80, 24);
    let (x, y) = cursor.expect("visible search cursor");
    assert!(x < 80 && y < 24);
    assert_eq!(buffer[(x, y)].symbol(), "b");
    assert_eq!(buffer[(x - 3, y)].symbol(), "型");
    assert_eq!(buffer[(x - 5, y)].symbol(), "模");
}

#[tokio::test]
async fn long_model_query_scrolls_with_cursor() {
    let mut tui = harness();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, &format!("HEAD{}TAIL", "模型".repeat(40))).await;
    let (screen, cursor) = tui.screen_with_cursor(80, 24);
    let (x, y) = cursor.expect("visible search cursor");
    assert!(
        screen
            .lines()
            .nth(y as usize)
            .expect("cursor row")
            .contains("TAIL"),
        "{screen}"
    );
    assert!(x < 80 && y < 24);
    press(&mut tui, KeyCode::Home).await;
    let (screen, cursor) = tui.screen_with_cursor(80, 24);
    let (_, y) = cursor.expect("visible search cursor");
    assert!(
        screen
            .lines()
            .nth(y as usize)
            .expect("cursor row")
            .contains("HEAD"),
        "{screen}"
    );
}
