use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use rara_persistence::prompt_history::{PromptHistoryEntry, PromptHistoryStore};

use super::state::{HelpTab, Overlay};
use super::testing::TuiHarness;

fn attach(tui: &mut TuiHarness) -> PromptHistoryStore {
    tui.app_mut().attach_prompt_history();
    PromptHistoryStore::new(tui.app().config_manager.path.parent().unwrap())
}

async fn loaded(tui: &mut TuiHarness) {
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while tui.app().prompt_history_loading() {
            tui.app_mut().poll_prompt_history();
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("history load timeout");
}

async fn key(tui: &mut TuiHarness, code: KeyCode) {
    tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
        .await
        .unwrap();
}

async fn search(tui: &mut TuiHarness) {
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
}

#[tokio::test]
async fn reverse_search_preserves_draft_and_accepts_without_submitting() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().record_input_history("older request");
    tui.app_mut().bottom_pane.input = "unfinished draft".into();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(3);
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, "unfinished draft");
    for c in "older".chars() {
        tui.press_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE))
            .await
            .unwrap();
    }
    assert_eq!(tui.app().bottom_pane.input, "unfinished draft");
    tui.press_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, "unfinished draft");
    assert_eq!(tui.app().bottom_pane.input_cursor_offset, Some(3));
    tui.press_key(KeyEvent::new(KeyCode::Char('r'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    tui.press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, "older request");
    tui.expect_no_commands();
}

#[tokio::test]
async fn restart_and_new_search_observe_shared_history_without_duplicates() {
    let mut first = TuiHarness::new(Default::default()).unwrap();
    let store = attach(&mut first);
    first.app_mut().record_input_history("first session prompt");
    first.app_mut().shutdown_prompt_history().await.unwrap();
    let mut second = TuiHarness::new(Default::default()).unwrap();
    second.app_mut().config_manager.path = first.app().config_manager.path.clone();
    attach(&mut second);
    assert!(second.app().input_history.is_empty());
    key(&mut second, KeyCode::Up).await;
    loaded(&mut second).await;
    assert_eq!(second.app().bottom_pane.input, "first session prompt");
    key(&mut second, KeyCode::Down).await;
    assert!(second.app().bottom_pane.input.is_empty());
    store
        .append(&PromptHistoryEntry::new("another session prompt").unwrap())
        .unwrap();
    search(&mut second).await;
    loaded(&mut second).await;
    assert_eq!(
        second.app().history_matches(),
        vec!["another session prompt", "first session prompt"]
    );
    key(&mut second, KeyCode::Enter).await;
    assert_eq!(second.app().bottom_pane.input, "another session prompt");
    second.expect_no_commands();
    second.app_mut().shutdown_prompt_history().await.unwrap();
}

#[tokio::test]
async fn real_submit_filters_history_without_changing_the_submitted_prompt() {
    use super::runtime_port::RuntimeCommand;
    use crate::runtime_control::InputControlRequest;
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let store = attach(&mut tui);
    let mut commands = Vec::new();
    for input in [
        " private prompt",
        "inspect token=0123456789abcdef",
        "normal prompt",
    ] {
        tui.app_mut().bottom_pane.input = input.into();
        key(&mut tui, KeyCode::Enter).await;
        commands.push(RuntimeCommand::Input(
            InputControlRequest::SubmitUserPrompt {
                prompt: input.trim().into(),
            },
        ));
        tui.expect_commands(&commands);
    }
    let paste = "x".repeat(1200);
    tui.send_terminal_event(Event::Paste(paste.clone()))
        .await
        .unwrap();
    key(&mut tui, KeyCode::Enter).await;
    commands.push(RuntimeCommand::Input(
        InputControlRequest::SubmitUserPrompt { prompt: paste },
    ));
    tui.expect_commands(&commands);
    tui.app_mut().shutdown_prompt_history().await.unwrap();
    let loaded = store.load().unwrap();
    let history: Vec<_> = loaded
        .entries
        .iter()
        .map(PromptHistoryEntry::text)
        .collect();
    assert_eq!(
        history,
        vec!["inspect token=[REDACTED_SECRET]", "normal prompt"]
    );
    assert_eq!(tui.app().input_history, history);
    let bytes = std::fs::read_to_string(store.path()).unwrap();
    assert!(!bytes.contains("0123456789abcdef"));
    assert!(!bytes.contains("private prompt"));
    assert!(!bytes.contains("Pasted Content"));
}

#[tokio::test]
async fn disabling_persistence_keeps_local_recall_without_touching_existing_history() {
    for existing in [false, true] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        tui.app_mut().config.tui.history.enabled = false;
        let store = attach(&mut tui);
        let bytes = if existing {
            store
                .append(&PromptHistoryEntry::new("another session").unwrap())
                .unwrap();
            Some(std::fs::read(store.path()).unwrap())
        } else {
            None
        };
        tui.app_mut().record_input_history("local only");
        search(&mut tui).await;
        assert!(!tui.app().prompt_history_loading());
        assert_eq!(tui.app().history_matches(), vec!["local only"]);
        key(&mut tui, KeyCode::Enter).await;
        assert_eq!(tui.app().bottom_pane.input, "local only");
        tui.app_mut().shutdown_prompt_history().await.unwrap();
        if let Some(bytes) = bytes {
            assert_eq!(std::fs::read(store.path()).unwrap(), bytes);
        } else {
            assert!(!store.path().exists());
            assert!(!store.path().with_file_name("prompt_history.lock").exists());
        }
    }
}

#[tokio::test]
async fn a_slow_read_cannot_overwrite_a_new_draft_or_overlay() {
    use fs2::FileExt;
    for replace_overlay in [false, true] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        let store = attach(&mut tui);
        store
            .append(&PromptHistoryEntry::new("old prompt").unwrap())
            .unwrap();
        let lock = std::fs::File::options()
            .read(true)
            .write(true)
            .open(store.path().with_file_name("prompt_history.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        tui.app_mut().record_input_history("queued prompt");
        key(&mut tui, KeyCode::Up).await;
        assert!(tui.app().prompt_history_loading());
        tokio::time::timeout(
            std::time::Duration::from_millis(200),
            key(&mut tui, KeyCode::Char('x')),
        )
        .await
        .expect("input must not wait for disk");
        if replace_overlay {
            tui.app_mut().open_overlay(Overlay::Help(HelpTab::General));
        }
        drop(lock);
        loaded(&mut tui).await;
        assert_eq!(tui.app().bottom_pane.input, "x");
        assert_eq!(
            tui.app().overlay,
            replace_overlay.then_some(Overlay::Help(HelpTab::General))
        );
        tui.app_mut().shutdown_prompt_history().await.unwrap();
    }
}

#[tokio::test]
async fn pending_navigation_keeps_up_down_order_without_losing_the_draft() {
    use fs2::FileExt;
    for steps in [1, 2] {
        let mut tui = TuiHarness::new(Default::default()).unwrap();
        let store = attach(&mut tui);
        tui.app_mut().record_input_history("older");
        tui.app_mut().shutdown_prompt_history().await.unwrap();
        let lock = std::fs::File::options()
            .read(true)
            .write(true)
            .open(store.path().with_file_name("prompt_history.lock"))
            .unwrap();
        lock.lock_exclusive().unwrap();
        tui.app_mut().record_input_history("newest");
        tui.app_mut().bottom_pane.input = "original draft".into();
        tui.app_mut().bottom_pane.input_cursor_offset = Some(0);
        for _ in 0..steps {
            key(&mut tui, KeyCode::Up).await;
        }
        key(&mut tui, KeyCode::Down).await;
        drop(lock);
        loaded(&mut tui).await;
        assert_eq!(
            tui.app().bottom_pane.input,
            if steps == 1 {
                "original draft"
            } else {
                "newest"
            }
        );
        if steps == 1 {
            assert_eq!(tui.app().bottom_pane.input_cursor_offset, Some(0));
        } else {
            key(&mut tui, KeyCode::Down).await;
            assert_eq!(tui.app().bottom_pane.input, "original draft");
        }
        tui.app_mut().shutdown_prompt_history().await.unwrap();
    }
}

#[tokio::test]
async fn refresh_preserves_explicit_selection_and_newer_local_submissions() {
    use fs2::FileExt;
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let store = attach(&mut tui);
    for text in ["oldest", "selected prompt"] {
        tui.app_mut().record_input_history(text);
    }
    tui.app_mut().shutdown_prompt_history().await.unwrap();
    store
        .append(&PromptHistoryEntry::new("external prompt").unwrap())
        .unwrap();
    let lock = std::fs::File::options()
        .read(true)
        .write(true)
        .open(store.path().with_file_name("prompt_history.lock"))
        .unwrap();
    lock.lock_exclusive().unwrap();
    tui.app_mut().record_input_history("queued prompt");
    search(&mut tui).await;
    search(&mut tui).await;
    assert_eq!(
        tui.app().history_matches()[tui.app().prompt_history.selected],
        "selected prompt"
    );
    tui.app_mut()
        .record_input_history("submitted while loading");
    drop(lock);
    loaded(&mut tui).await;
    assert_eq!(
        tui.app().input_history,
        [
            "oldest",
            "selected prompt",
            "external prompt",
            "queued prompt",
            "submitted while loading"
        ]
    );
    assert_eq!(
        tui.app().history_matches()[tui.app().prompt_history.selected],
        "selected prompt"
    );
    tui.app_mut().shutdown_prompt_history().await.unwrap();
    assert_eq!(store.load().unwrap().entries.len(), 5);
}

#[tokio::test]
async fn failed_writes_remain_recallable_and_retry_after_storage_recovers() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let store = attach(&mut tui);
    std::fs::create_dir(store.path()).unwrap();
    tui.app_mut()
        .record_input_history("retained pending prompt");
    search(&mut tui).await;
    loaded(&mut tui).await;
    assert_eq!(tui.app().history_matches(), vec!["retained pending prompt"]);
    assert!(
        tui.app()
            .bottom_pane
            .notice
            .as_deref()
            .unwrap()
            .contains("Could not read prompt history")
    );
    key(&mut tui, KeyCode::Esc).await;
    key(&mut tui, KeyCode::Up).await;
    loaded(&mut tui).await;
    assert_eq!(tui.app().bottom_pane.input, "retained pending prompt");
    std::fs::remove_dir(store.path()).unwrap();
    search(&mut tui).await;
    loaded(&mut tui).await;
    assert_eq!(tui.app().history_matches(), vec!["retained pending prompt"]);
    tui.app_mut().shutdown_prompt_history().await.unwrap();
    assert_eq!(
        store.load().unwrap().entries[0].text(),
        "retained pending prompt"
    );
}

#[tokio::test]
async fn search_edits_unicode_and_traverses_unique_matches_without_wrapping() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    for text in ["first request", "second request", "first request"] {
        tui.app_mut().record_input_history(text);
    }
    search(&mut tui).await;
    assert_eq!(
        tui.app().history_matches(),
        vec!["first request", "second request"]
    );
    search(&mut tui).await;
    assert_eq!(tui.app().prompt_history.selected, 1);
    search(&mut tui).await;
    assert_eq!(tui.app().prompt_history.selected, 1);
    key(&mut tui, KeyCode::Down).await;
    assert_eq!(tui.app().prompt_history.selected, 0);
    tui.send_terminal_event(Event::Paste("SECOND\na\u{301}".into()))
        .await
        .unwrap();
    assert_eq!(tui.app().prompt_history.query, "SECOND a\u{301}");
    key(&mut tui, KeyCode::Backspace).await;
    key(&mut tui, KeyCode::Backspace).await;
    assert_eq!(tui.app().prompt_history.query, "SECOND");
    assert_eq!(tui.app().history_matches(), vec!["second request"]);
    key(&mut tui, KeyCode::Char('!')).await;
    key(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().overlay, Some(Overlay::HistorySearch));
    assert!(tui.app().bottom_pane.input.is_empty());
    tui.expect_no_commands();
}

#[tokio::test]
async fn cancelling_search_preserves_large_paste_payload_and_cursor() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().record_input_history("old prompt");
    tui.send_terminal_event(Event::Paste("p".repeat(1200)))
        .await
        .unwrap();
    tui.app_mut().flush_composer_paste();
    tui.app_mut().bottom_pane.input_cursor_offset = Some(0);
    let draft = tui.app().bottom_pane.input.clone();
    let pending = tui.app().bottom_pane.large_paste_pending.clone();
    search(&mut tui).await;
    key(&mut tui, KeyCode::Char('x')).await;
    tui.press_key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL))
        .await
        .unwrap();
    assert_eq!(tui.app().bottom_pane.input, draft);
    assert_eq!(tui.app().bottom_pane.input_cursor_offset, Some(0));
    assert_eq!(tui.app().bottom_pane.large_paste_pending, pending);
    tui.expect_no_commands();
}

#[tokio::test]
async fn history_search_render_shows_query_selection_and_multiline_preview() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().snapshot.cwd = "<CWD>".into();
    tui.app_mut().record_input_history("older prompt");
    tui.app_mut().record_input_history(
        "inspect the query plan\nKeep the index lookup and compare row counts.",
    );
    tui.app_mut().bottom_pane.input = "unfinished draft".into();
    search(&mut tui).await;
    insta::assert_snapshot!("prompt_history_search", tui.screen_text(90, 28));
    for query in [
        "",
        "a\u{301} \u{754c}\u{754c} query with multiple wrapped words",
        "left\u{202e}right",
    ] {
        tui.app_mut().prompt_history.query = query.into();
        for (width, height) in [(1, 1), (12, 5), (24, 8), (50, 12)] {
            let (screen, cursor) = tui.screen_with_cursor(width, height);
            if let Some((x, y)) = cursor {
                assert!(
                    x < width && y < height,
                    "cursor outside {width}x{height}: {cursor:?}"
                );
            }
            assert!(!screen.contains('\u{202e}'));
            assert_eq!(tui.app().bottom_pane.input, "unfinished draft");
        }
    }
}
