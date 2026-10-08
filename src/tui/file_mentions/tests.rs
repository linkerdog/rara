use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use rara_file_search::FileSearchResults;

use super::*;
use crate::runtime_control::InputControlRequest;
use crate::tui::runtime_port::RuntimeCommand;
use crate::tui::state::{HelpTab, Overlay};
use crate::tui::testing::TuiHarness;

fn response(generation: u64, paths: &[&str]) -> SearchResponse {
    SearchResponse {
        generation,
        result: Ok(IndexedSearchResults {
            results: FileSearchResults {
                matches: paths
                    .iter()
                    .map(|path| FileMatch {
                        root: PathBuf::from("/workspace"),
                        path: path.into(),
                        score: 1,
                        match_type: rara_file_search::MatchType::File,
                    })
                    .collect(),
                total_match_count: paths.len(),
                scanned_entry_count: paths.len(),
                truncated: false,
            },
            index_truncated: false,
            skipped_non_utf8: 0,
        }),
    }
}

fn results(tui: &mut TuiHarness, paths: &[&str]) {
    let app = tui.app_mut();
    app.refresh_file_mentions();
    let generation = app.file_mentions.query.as_ref().unwrap().generation;
    assert!(app.apply_file_response(response(generation, paths)));
}

async fn key(tui: &mut TuiHarness, code: KeyCode) {
    assert!(
        !tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn completion_accepts_without_submitting_and_preserves_canonical_history() {
    for accept in [KeyCode::Tab, KeyCode::Enter] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        tui.app_mut().set_input("inspect @a".into());
        results(&mut tui, &["a.rs", "src/a b\"\\\u{754c}.rs"]);
        key(&mut tui, KeyCode::Down).await;
        key(&mut tui, accept).await;
        let prompt = format!("inspect {}", encode_mention("src/a b\"\\\u{754c}.rs"));
        assert_eq!(tui.app().bottom_pane.input, format!("{prompt} "));
        assert!(!tui.app().file_mention_open());
        tui.expect_no_commands();
        key(&mut tui, KeyCode::Enter).await;
        tui.expect_commands(&[RuntimeCommand::Input(
            InputControlRequest::SubmitUserPrompt {
                prompt: prompt.clone(),
            },
        )]);
        key(&mut tui, KeyCode::Up).await;
        assert_eq!(tui.app().bottom_pane.input, prompt);
        assert_eq!(tui.app().bottom_pane.composer_atoms().len(), 1);
        key(&mut tui, KeyCode::Backspace).await;
        assert_eq!(tui.app().bottom_pane.input, "inspect ");
    }
}

#[tokio::test]
async fn loading_empty_and_dismissed_popups_keep_the_draft() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    key(&mut tui, KeyCode::Char('@')).await;
    assert!(tui.app().file_mention_open());
    for code in [KeyCode::Enter, KeyCode::Tab] {
        key(&mut tui, code).await;
    }
    assert_eq!(tui.app().bottom_pane.input, "@");
    results(&mut tui, &[]);
    key(&mut tui, KeyCode::Enter).await;
    key(&mut tui, KeyCode::Esc).await;
    assert!(!tui.app().file_mention_open());
    assert!(!tui.app_mut().refresh_file_mentions());
    assert_eq!(tui.app().bottom_pane.input, "@");
    key(&mut tui, KeyCode::Char('x')).await;
    assert!(tui.app().file_mention_open());
    assert!(
        !tui.press_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
            .await
            .unwrap()
    );
    assert_eq!(tui.app().bottom_pane.input, "@x");
    assert!(!tui.app().file_mention_open());
    tui.expect_no_commands();
}

#[test]
fn stale_results_are_fenced_by_generation_draft_cursor_session_root_and_overlay() {
    for mutation in 0..6 {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        let app = tui.app_mut();
        app.set_input("inspect @src".into());
        app.refresh_file_mentions();
        let generation = app.file_mentions.query.as_ref().unwrap().generation;
        match mutation {
            0 => app.file_mentions.query.as_mut().unwrap().generation += 1,
            1 => app.bottom_pane.input.push('x'),
            2 => app.bottom_pane.input_cursor_offset = Some(9),
            3 => app.snapshot.session_id = "new-session".into(),
            4 => app.snapshot.cwd = "/new-workspace".into(),
            5 => app.open_overlay(Overlay::Help(HelpTab::General)),
            _ => unreachable!(),
        }
        assert!(!app.apply_file_response(response(generation, &["obsolete.rs"])));
        assert!(app.file_mentions.matches.is_empty());
    }
}

#[test]
fn mention_detection_excludes_emails_and_atoms_and_replaces_the_whole_query() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    for text in ["name@example.com", "@\"src/a b.rs\"", "plain text", "@@"] {
        tui.app_mut().set_input(text.into());
        assert!(mention_query(tui.app()).is_none(), "{text}");
    }
    tui.app_mut().set_input("inspect @source tail".into());
    tui.app_mut().bottom_pane.input_cursor_offset = Some(11);
    assert_eq!(mention_query(tui.app()), Some((8..15, "so".into())));
    results(&mut tui, &["source.rs"]);
    tui.app_mut().apply_file_mention(FileMentionAction::Accept);
    assert_eq!(tui.app().bottom_pane.input, "inspect @\"source.rs\"  tail");
}

#[tokio::test(start_paused = true)]
async fn debounce_replaces_requests_and_closing_cancels_pending_discovery() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    key(&mut tui, KeyCode::Char('@')).await;
    assert!(!tui.app_mut().poll_file_mentions());
    tokio::time::advance(DEBOUNCE - Duration::from_millis(1)).await;
    key(&mut tui, KeyCode::Char('a')).await;
    tokio::time::advance(Duration::from_millis(1)).await;
    tui.app_mut().poll_file_mentions();
    assert!(tui.app().file_mentions.worker.is_none());
    key(&mut tui, KeyCode::Esc).await;
    tokio::time::advance(DEBOUNCE).await;
    tui.app_mut().poll_file_mentions();
    assert!(tui.app().file_mentions.worker.is_none());
}

#[tokio::test]
async fn large_pastes_and_mentions_survive_draft_history_navigation_and_submit() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().record_input_history("previous prompt");
    tui.app_mut()
        .set_input(format!("{} ", encode_mention("src/main.rs")));
    let payload = "many lines\n".repeat(150);
    tui.send_terminal_event(Event::Paste(payload.clone()))
        .await
        .unwrap();
    tui.app_mut().flush_composer_paste();
    let draft = tui.app().bottom_pane.input.clone();
    let atoms = tui.app().bottom_pane.composer_atoms();
    key(&mut tui, KeyCode::Home).await;
    let cursor = tui.app().composer_cursor_offset();
    key(&mut tui, KeyCode::Up).await;
    assert_eq!(tui.app().bottom_pane.input, "previous prompt");
    key(&mut tui, KeyCode::Down).await;
    assert_eq!(tui.app().bottom_pane.input, draft);
    assert_eq!(tui.app().composer_cursor_offset(), cursor);
    assert_eq!(tui.app().bottom_pane.composer_atoms(), atoms);
    key(&mut tui, KeyCode::End).await;
    key(&mut tui, KeyCode::Left).await;
    assert_eq!(tui.app().composer_cursor_offset(), atoms[1].range.start);
    key(&mut tui, KeyCode::Right).await;
    assert_eq!(tui.app().composer_cursor_offset(), atoms[1].range.end);
    key(&mut tui, KeyCode::Enter).await;
    tui.expect_commands(&[RuntimeCommand::Input(
        InputControlRequest::SubmitUserPrompt {
            prompt: format!("{} {}", encode_mention("src/main.rs"), payload.trim_end()),
        },
    )]);
}

#[tokio::test]
async fn history_search_takes_focus_and_failure_is_visible_without_submitting() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    key(&mut tui, KeyCode::Char('@')).await;
    let generation = tui.app().file_mentions.query.as_ref().unwrap().generation;
    assert!(tui.app_mut().apply_file_response(SearchResponse {
        generation,
        result: Err(anyhow::anyhow!("fixture read failure"))
    }));
    let screen = tui.screen_text(80, 24);
    assert!(screen.contains("Search failed"));
    assert!(screen.contains("fixture read failure"));
    key(&mut tui, KeyCode::Enter).await;
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(tui.app().overlay, Some(Overlay::HistorySearch));
    assert!(!tui.app().file_mention_open());
    key(&mut tui, KeyCode::Esc).await;
    assert_eq!(tui.app().bottom_pane.input, "@");
    tui.expect_no_commands();
}

#[test]
fn popup_rendering_and_tiny_viewports_keep_cursor_in_bounds() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().snapshot.cwd = "<CWD>".into();
    tui.app_mut().set_input("inspect @src".into());
    results(
        &mut tui,
        &["src/main.rs", "src/a b.rs", "src/\u{202e}unsafe.rs"],
    );
    insta::assert_snapshot!("file_mentions_picker", tui.screen_text(70, 18));
    for (width, height) in [(1, 1), (2, 2), (8, 3), (20, 5), (120, 24)] {
        let (_, cursor) = tui.screen_with_cursor(width, height);
        assert!(
            cursor.is_none_or(|(x, y)| x < width && y < height),
            "{width}x{height}: {cursor:?}"
        );
    }
}

#[tokio::test]
async fn paste_followed_by_enter_opens_completion_before_submit_routing() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.send_terminal_event(Event::Paste("inspect @src".into()))
        .await
        .unwrap();
    key(&mut tui, KeyCode::Enter).await;
    assert!(tui.app().file_mention_open());
    assert_eq!(tui.app().bottom_pane.input, "inspect @src");
    tui.expect_no_commands();
}

#[tokio::test]
async fn edits_clear_dismissal_even_when_the_query_returns_to_the_original_text() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    key(&mut tui, KeyCode::Char('@')).await;
    key(&mut tui, KeyCode::Esc).await;
    key(&mut tui, KeyCode::Char('x')).await;
    key(&mut tui, KeyCode::Backspace).await;
    assert!(tui.app().file_mention_open());
}

#[tokio::test]
async fn persisted_quoted_paths_are_atomic_after_reverse_search_recall() {
    use rara_persistence::prompt_history::{PromptHistoryEntry, PromptHistoryStore};
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let prompt = format!("inspect {}", encode_mention("src/a b.rs"));
    let store = PromptHistoryStore::new(tui.app().config_manager.path.parent().unwrap());
    store
        .append(&PromptHistoryEntry::new(&prompt).unwrap())
        .unwrap();
    tui.app_mut().attach_prompt_history();
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while tui.app().prompt_history_loading() {
            tui.app_mut().poll_prompt_history();
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("history worker completion");
    key(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().bottom_pane.input, prompt);
    assert_eq!(tui.app().bottom_pane.composer_atoms().len(), 1);
    key(&mut tui, KeyCode::Left).await;
    assert_eq!(tui.app().composer_cursor_offset(), 8);
    key(&mut tui, KeyCode::Delete).await;
    assert_eq!(tui.app().bottom_pane.input, "inspect ");
    tui.expect_no_commands();
    tui.app_mut().shutdown_prompt_history().await.unwrap();
}

#[test]
fn pending_decisions_take_focus_and_reject_search_results() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().set_input("@src".into());
    results(&mut tui, &["src/main.rs"]);
    let generation = tui.app().file_mentions.query.as_ref().unwrap().generation;
    tui.app_mut().show_pending_plan_approval(None);
    assert!(!tui.app().file_mention_open());
    assert!(
        !tui.app_mut()
            .apply_file_response(response(generation, &["obsolete.rs"]))
    );
    tui.app_mut().apply_file_mention(FileMentionAction::Accept);
    assert_eq!(tui.app().bottom_pane.input, "@src");
    assert!(tui.app().file_mentions.query.is_none());
}

#[tokio::test]
async fn popup_mouse_input_cannot_select_hidden_transcript_rows() {
    use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    key(&mut tui, KeyCode::Char('@')).await;
    for kind in [
        MouseEventKind::Down(MouseButton::Left),
        MouseEventKind::Drag(MouseButton::Left),
        MouseEventKind::ScrollUp,
    ] {
        tui.send_terminal_event(Event::Mouse(MouseEvent {
            kind,
            column: 3,
            row: 2,
            modifiers: KeyModifiers::NONE,
        }))
        .await
        .unwrap();
        assert!(!tui.app().transcript_selection.is_dragging());
    }
    assert_eq!(tui.app().bottom_pane.input, "@");
    tui.expect_no_commands();
}
