use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use super::state::{
    HelpTab, Overlay, ProviderFamily, RuntimeSnapshot, SkillPickerEntry, StatusTab,
};
use super::testing::TuiHarness;
use super::{RuntimeCommand, RuntimeMaintenanceCommand};
use crate::config::OpenAiEndpointKind;

fn harness() -> TuiHarness {
    TuiHarness::new(RuntimeSnapshot::default()).expect("isolated TUI harness")
}

async fn press(harness: &mut TuiHarness, code: KeyCode) {
    assert!(
        !harness
            .press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .expect("dispatch key"),
        "interaction must not quit"
    );
}

async fn type_text(harness: &mut TuiHarness, text: &str) {
    for ch in text.chars() {
        press(harness, KeyCode::Char(ch)).await;
    }
}

// CMD-01: inspect the production palette, not a test-only help formatter.
#[tokio::test]
async fn palette_renders_canonical_commands_without_duplicate_aliases() {
    let mut tui = harness();
    type_text(&mut tui, "/").await;
    let screen = tui.screen_text(120, 64);

    for command in ["/status", "/context", "/resume", "/model", "/mem"] {
        assert!(screen.contains(command), "missing {command}:\n{screen}");
    }
    for alias in ["/runtime", "/memory", "/threads"] {
        assert!(!screen.contains(alias), "duplicate {alias}:\n{screen}");
    }
    assert!(screen.contains("18 commands"), "{screen}");
}

#[tokio::test]
async fn complete_alias_in_palette_opens_its_canonical_surface() {
    for (alias, expected) in [
        ("/runtime", Overlay::Status(StatusTab::Overview)),
        ("/memory", Overlay::Context),
    ] {
        let mut tui = harness();
        type_text(&mut tui, alias).await;
        press(&mut tui, KeyCode::Enter).await;
        assert_eq!(tui.app().overlay, Some(expected));
        tui.expect_no_commands();
    }
}

// CMD-04: /help opens General, so that is the page that must reject stale copy.
#[tokio::test]
async fn help_general_renders_current_commands_and_keyboard_guidance() {
    let mut tui = harness();
    type_text(&mut tui, "/help").await;
    press(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().overlay, Some(Overlay::Help(HelpTab::General)));
    let screen = tui.screen_text(120, 64);

    for obsolete in ["/login", "/logout", "replace_lines", "cycles through"] {
        assert!(!screen.contains(obsolete), "obsolete {obsolete}:\n{screen}");
    }
    for current in ["/connect", "/model", "/permissions", "Ctrl+C", "Esc"] {
        assert!(screen.contains(current), "missing {current}:\n{screen}");
    }
}

#[tokio::test]
async fn help_commands_can_reach_entries_below_the_viewport() {
    let mut tui = harness();
    type_text(&mut tui, "/help").await;
    press(&mut tui, KeyCode::Enter).await;
    press(&mut tui, KeyCode::Char('2')).await;
    assert!(!tui.screen_text(100, 24).contains("/tasks"));
    for _ in 0..30 {
        press(&mut tui, KeyCode::Down).await;
    }
    let screen = tui.screen_text(100, 24);
    assert!(
        screen.contains("/tasks"),
        "last command is unreachable:\n{screen}"
    );
    for alias in ["/runtime", "/memory", "/threads"] {
        assert!(!screen.contains(alias), "duplicate {alias}:\n{screen}");
    }
}

// INPUT-01: printable letters belong to the search field.
#[tokio::test]
async fn command_search_keeps_j_and_k_as_text() {
    let mut tui = harness();
    type_text(&mut tui, "/skills").await;
    assert_eq!(tui.app().bottom_pane.input, "/skills");
    assert!(tui.screen_text(100, 30).contains("/skills"));
    type_text(&mut tui, "j").await;
    assert_eq!(tui.app().bottom_pane.input, "/skillsj");
    press(&mut tui, KeyCode::Backspace).await;
    press(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().overlay.is_none());
    assert!(tui.app().bottom_pane.input.is_empty());
    tui.expect_no_commands();
}

#[tokio::test]
async fn model_search_keeps_j_and_k_as_text() {
    let mut tui = harness();
    tui.app_mut().open_overlay(Overlay::ModelSearch);
    type_text(&mut tui, "jk").await;
    assert_eq!(tui.app().model_search_query, "jk");
    assert!(tui.screen_text(100, 30).contains("jk"));
    press(&mut tui, KeyCode::Esc).await;
    assert!(tui.app().model_search_query.is_empty());
    assert!(tui.app().overlay.is_none());
    tui.expect_no_commands();
}

// CMD-02: selecting help syntax must never change the active task-list ID.
#[tokio::test]
async fn selecting_tasks_does_not_submit_the_usage_placeholder() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.snapshot.shared_tasks.task_list_id = "review-work".into();
    app.bottom_pane.input = "/tasks".into();
    app.sync_command_palette_with_input();

    press(&mut tui, KeyCode::Enter).await;

    assert_eq!(tui.app().snapshot.shared_tasks.task_list_id, "review-work");
    let screen = tui.screen_text(100, 30);
    assert!(
        screen.contains("Active shared task list: review-work"),
        "{screen}"
    );
    assert!(!screen.contains("[task_list_id]"), "{screen}");
    tui.expect_no_commands();
}

// INPUT-04: rendering and Enter must consume the same filtered model rows.
#[tokio::test]
async fn model_provider_search_renders_and_selects_the_same_rows() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.provider_connection_status.clear();
    app.provider_connection_status
        .insert(ProviderFamily::DeepSeek, true);
    app.config.set_provider("deepseek");
    app.config.set_api_key("test-deepseek-key");
    app.set_deepseek_model_options(vec!["alpha-chat".into(), "beta-chat".into()]);
    app.open_overlay(Overlay::ModelSearch);
    app.model_search_query = "DeepSeek".into();
    app.model_search_idx = 0;

    let screen = tui.screen_text(100, 30);
    assert!(screen.contains("alpha-chat"), "{screen}");
    assert!(screen.contains("beta-chat"), "{screen}");
    press(&mut tui, KeyCode::Down).await;
    assert_eq!(tui.app().model_search_idx, 1);
    press(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().config.model.as_deref(), Some("beta-chat"));
    assert!(tui.app().overlay.is_none());
    tui.expect_command(RuntimeCommand::Maintenance(
        RuntimeMaintenanceCommand::Rebuild,
    ));
}

#[tokio::test]
async fn model_search_selection_requests_runtime_rebuild() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.provider_connection_status.clear();
    app.provider_connection_status
        .insert(ProviderFamily::DeepSeek, true);
    app.config.set_provider("deepseek");
    app.config.set_api_key("test-deepseek-key");
    app.set_deepseek_model_options(vec!["alpha-chat".into()]);
    app.open_overlay(Overlay::ModelSearch);
    app.model_search_idx = 0;

    press(&mut tui, KeyCode::Enter).await;

    assert_eq!(tui.app().config.model.as_deref(), Some("alpha-chat"));
    tui.expect_command(RuntimeCommand::Maintenance(
        RuntimeMaintenanceCommand::Rebuild,
    ));
}

#[tokio::test]
async fn model_search_preserves_profile_identity_for_shared_model_names() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.config.openai_profiles.clear();
    for (id, label) in [("alpha", "First endpoint"), ("beta", "Second endpoint")] {
        app.config
            .select_openai_profile(id, label, OpenAiEndpointKind::Custom);
        app.config.set_model(Some("shared-model".into()));
        app.config.set_api_key("test-endpoint-key");
        app.config
            .set_base_url(Some("http://127.0.0.1:1234/v1".into()));
    }
    app.provider_connection_status.clear();
    app.provider_connection_status
        .insert(ProviderFamily::OpenAiCompatible, true);
    app.open_overlay(Overlay::ModelSearch);
    app.model_search_query = "Second endpoint".into();
    app.model_search_idx = 0;

    press(&mut tui, KeyCode::Enter).await;

    assert_eq!(tui.app().config.active_openai_profile_id(), Some("beta"));
    tui.expect_command(RuntimeCommand::Maintenance(
        RuntimeMaintenanceCommand::Rebuild,
    ));
}

#[tokio::test]
async fn empty_model_search_does_not_select_an_invisible_model() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.provider_connection_status.clear();
    app.open_overlay(Overlay::ModelSearch);
    let model = app.config.model.clone();

    assert!(
        tui.screen_text(100, 30)
            .contains("No available provider models")
    );
    press(&mut tui, KeyCode::Down).await;
    press(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().config.model, model);
    assert_eq!(tui.app().overlay, Some(Overlay::ModelSearch));
    tui.expect_no_commands();
}

#[tokio::test]
async fn skills_inspection_does_not_offer_an_unwired_toggle() {
    let mut tui = harness();
    let app = tui.app_mut();
    app.skill_picker_entries = vec![SkillPickerEntry {
        name: "reviewer".into(),
        title: "Review changes".into(),
        scope: "cwd".into(),
        disable_model_invocation: false,
    }];
    app.open_overlay(Overlay::SkillsPicker);

    press(&mut tui, KeyCode::Char(' ')).await;

    assert!(!tui.app().skill_picker_entries[0].disable_model_invocation);
    let screen = tui.screen_text(100, 30);
    assert!(screen.contains("reviewer"), "{screen}");
    assert!(screen.contains("Read-only"), "{screen}");
    assert!(!screen.contains("Toggle"), "{screen}");
    assert!(!screen.contains("Space toggle"), "{screen}");
    tui.expect_no_commands();
}
