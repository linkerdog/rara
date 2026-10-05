use super::*;
use crate::tui::composer_atoms::encode_mention;
use crate::tui::state::Overlay;
use crate::tui::testing::TuiHarness;

#[tokio::test]
async fn failure_and_unchanged_result_keep_cursor_and_owned_pastes() {
    for fail in [false, true] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        tui.app_mut()
            .set_input(format!("inspect {} ", encode_mention("a b.rs")));
        let payload = "full paste body\n".repeat(100);
        tui.app_mut().bottom_pane.handle_paste_burst_chunk(&payload);
        let (draft, request) = EditorDraft::capture(tui.app_mut());
        let before = tui.app().bottom_pane.saved_draft();
        assert_eq!(
            &*request.seed,
            format!("inspect {} {payload}", encode_mention("a b.rs"))
        );
        let result = if fail {
            Err(anyhow::anyhow!("fixture editor failed"))
        } else {
            Ok(request.seed.to_string())
        };
        draft.finish(tui.app_mut(), result).await;
        assert_eq!(tui.app().bottom_pane.saved_draft(), before);
        tui.expect_no_commands();
    }
}

#[tokio::test]
async fn successful_edit_is_sanitized_without_submitting_or_editing_another_overlay() {
    for overlay in [None, Some(Overlay::ModelSearch)] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        let (draft, _) = EditorDraft::capture(tui.app_mut());
        if let Some(overlay) = overlay {
            tui.app_mut().open_overlay(overlay);
        }
        draft
            .finish(
                tui.app_mut(),
                Ok("\x1b[31medited\x1b[0m\r\n@\"a b.rs\"".into()),
            )
            .await;
        assert_eq!(tui.app().bottom_pane.input, "edited\n@\"a b.rs\"");
        assert_eq!(tui.app().bottom_pane.composer_atoms().len(), 1);
        assert_eq!(tui.app().overlay, overlay);
        assert!(tui.app().model_search_query.is_empty());
        tui.expect_no_commands();
    }
}

#[tokio::test]
async fn stale_session_workspace_or_draft_saves_recovery_without_overwriting_current_text() {
    for change in 0..3 {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        tui.app_mut().set_input("original".into());
        let (draft, _) = EditorDraft::capture(tui.app_mut());
        match change {
            0 => tui.app_mut().snapshot.session_id = "another-session".into(),
            1 => tui.app_mut().snapshot.cwd = "/another-workspace".into(),
            2 => tui.app_mut().set_input("new draft".into()),
            _ => unreachable!(),
        }
        let before = tui.app().bottom_pane.saved_draft();
        draft
            .finish(tui.app_mut(), Ok("edited content".into()))
            .await;
        assert_eq!(tui.app().bottom_pane.saved_draft(), before);
        let notice = tui.app().notice_text().unwrap();
        let path = notice.split("edited text saved to ").nth(1).unwrap();
        assert_eq!(std::fs::read_to_string(path).unwrap(), "edited content");
        std::fs::remove_file(path).unwrap();
    }
}

#[test]
fn editor_shortcut_has_composer_priority_and_ignores_repeats_and_decisions() {
    use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    use crate::tui::event_stream::{UiEvent, translate_event};
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    for overlay in [
        None,
        Some(Overlay::CommandPalette),
        Some(Overlay::HistorySearch),
        Some(Overlay::ModelSearch),
    ] {
        tui.app_mut().overlay = overlay;
        let key = KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL);
        assert_eq!(
            matches!(
                translate_event(Event::Key(key), tui.app_mut()),
                Some(UiEvent::ExternalEditor)
            ),
            matches!(overlay, None | Some(Overlay::CommandPalette))
        );
        let repeated = KeyEvent::new_with_kind(
            KeyCode::Char('g'),
            KeyModifiers::CONTROL,
            KeyEventKind::Repeat,
        );
        assert!(!matches!(
            translate_event(Event::Key(repeated), tui.app_mut()),
            Some(UiEvent::ExternalEditor)
        ));
    }
    tui.app_mut().overlay = None;
    tui.app_mut().show_pending_plan_approval(None);
    assert!(!matches!(
        translate_event(
            Event::Key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            tui.app_mut()
        ),
        Some(UiEvent::ExternalEditor)
    ));
}
