use super::*;
use crate::config::{ConfigManager, RaraConfig};
use crate::runtime_control::{ApprovalEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance};
use crate::tui::runtime::apply_tui_event;
use crate::tui::state::{PendingApprovalSnapshot, PendingInteractionSnapshot, TuiEvent};

fn app(method: TerminalNotificationMethod) -> TuiApp {
    let root = tempfile::tempdir().unwrap();
    let mut config = RaraConfig::default();
    config.tui.terminal.notifications = method;
    let mut app = TuiApp::with_config(
        ConfigManager {
            path: root.path().join("config.json"),
        },
        config,
    )
    .unwrap();
    app.snapshot.session_id = "12345678-1234-1234-1234-123456789abc".into();
    app.snapshot.cwd = "/workspace/project".into();
    app
}

fn approval(app: &mut TuiApp, id: &str) {
    apply_tui_event(
        app,
        TuiEvent::Runtime(Box::new(RuntimeControlEvent {
            event_id: format!("approval-{id}"),
            provenance: RuntimeProvenance::local_tui(&app.snapshot.session_id),
            turn_id: Some("turn".into()),
            sequence: 1,
            event: RuntimeEvent::Approval(ApprovalEvent::Requested {
                approval_id: id.into(),
                kind: "shell".into(),
            }),
        })),
    );
}

#[test]
fn notification_events_require_opt_in_and_unfocused_and_are_consumed_once() {
    for method in [
        TerminalNotificationMethod::Off,
        TerminalNotificationMethod::Bell,
        TerminalNotificationMethod::Osc9,
    ] {
        let mut app = app(method);
        let mut feedback = TerminalFeedback::new(TitleMode::Disabled, TerminalTarget::Direct);
        let mut bytes = Vec::new();
        approval(&mut app, "focused");
        app.terminal_focused = false;
        approval(&mut app, "focused");
        feedback.update(&mut app, &mut bytes).unwrap();
        assert!(bytes.is_empty(), "focused events cannot fire on blur");
        approval(&mut app, "new");
        feedback.update(&mut app, &mut bytes).unwrap();
        approval(&mut app, "new");
        feedback.update(&mut app, &mut bytes).unwrap();
        let expected = match method {
            TerminalNotificationMethod::Off => "",
            TerminalNotificationMethod::Bell => "\x07",
            TerminalNotificationMethod::Osc9 => "\x1b]9;RARA: approval required\x07",
        };
        assert_eq!(bytes, expected.as_bytes());
        approval(&mut app, "regained-focus");
        app.terminal_focused = true;
        feedback.update(&mut app, &mut bytes).unwrap();
        app.terminal_focused = false;
        feedback.update(&mut app, &mut bytes).unwrap();
        assert_eq!(bytes, expected.as_bytes());
    }
}

#[test]
fn completion_attention_priority_and_cross_session_fence() {
    let mut app = app(TerminalNotificationMethod::Osc9);
    app.terminal_focused = false;
    let mut feedback = TerminalFeedback::new(TitleMode::Disabled, TerminalTarget::Direct);
    let mut bytes = Vec::new();
    app.notify_terminal_query_complete();
    approval(&mut app, "pending");
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x1b]9;RARA: approval required\x07");
    bytes.clear();
    app.clear_terminal_attention();
    app.notify_terminal_query_complete();
    feedback.update(&mut app, &mut bytes).unwrap();
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x1b]9;RARA: turn complete\x07");
    bytes.clear();
    app.notify_terminal_query_complete();
    app.snapshot.session_id = "other".into();
    feedback.update(&mut app, &mut bytes).unwrap();
    assert!(bytes.is_empty());
}

#[test]
fn pending_approval_completion_does_not_repeat_live_event_or_emit_completion() {
    let mut app = app(TerminalNotificationMethod::Bell);
    app.terminal_focused = false;
    let mut feedback = TerminalFeedback::new(TitleMode::Disabled, TerminalTarget::Direct);
    let mut bytes = Vec::new();
    approval(&mut app, "command");
    feedback.update(&mut app, &mut bytes).unwrap();
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Approval".into(),
            summary: String::new(),
            options: Vec::new(),
            note: None,
            source: None,
            created_at_epoch_seconds: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "command".into(),
                ..Default::default()
            }),
        });
    app.notify_terminal_query_complete();
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x07");
}

#[test]
fn titles_are_sanitized_deduplicated_and_reemitted_after_suspend() {
    let mut app = app(TerminalNotificationMethod::Off);
    let mut feedback = TerminalFeedback::new(TitleMode::Enabled, TerminalTarget::Direct);
    let mut bytes = Vec::new();
    feedback.update(&mut app, &mut bytes).unwrap();
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x1b]2;[idle] project / 12345678 - RARA\x07");
    bytes.clear();
    app.set_terminal_thread_title(Some(
        "fix \u{202e}\x1b[31mparser\x1b[0m\x1b]9;injected\x07".into(),
    ));
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x1b]2;[idle] project / fix parser - RARA\x07");
    feedback.resumed();
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x1b]2;[idle] project / fix parser - RARA\x07\x1b]2;[idle] project / fix parser - RARA\x07");
    bytes.clear();
    approval(&mut app, "approval");
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(
        bytes,
        b"\x1b]2;[needs approval] project / fix parser - RARA\x07"
    );
    app.snapshot.session_id = "another-thread".into();
    assert_eq!(title_for(&app), "[idle] project / another- - RARA");
    assert_eq!(
        sanitize_title(&"\u{1f600}".repeat(300)).chars().count(),
        240
    );
}

#[test]
fn exact_transports_preserve_vt100_cells_and_cursor() {
    for (target, osc9) in [
        (TerminalTarget::Direct, "\x1b]9;RARA: turn complete\x07"),
        (
            TerminalTarget::Tmux,
            "\x1bPtmux;\x1b\x1b]9;RARA: turn complete\x07\x1b\\",
        ),
        (
            TerminalTarget::Screen,
            "\x1bP\x1b]9;RARA: turn complete\x07\x1b\\",
        ),
    ] {
        for method in [
            TerminalNotificationMethod::Bell,
            TerminalNotificationMethod::Osc9,
        ] {
            let mut app = app(method);
            app.terminal_focused = false;
            app.notify_terminal_query_complete();
            let mut feedback = TerminalFeedback::new(TitleMode::Disabled, target);
            let mut bytes = Vec::new();
            feedback.update(&mut app, &mut bytes).unwrap();
            assert_eq!(
                bytes,
                if method == TerminalNotificationMethod::Bell {
                    b"\x07".as_slice()
                } else {
                    osc9.as_bytes()
                }
            );
            let mut parser = vt100::Parser::new(4, 40, 0);
            parser.process(b"existing frame\x1b[3;5H");
            let contents = parser.screen().contents();
            parser.process(&bytes);
            parser.process(b"\x1b[22;2t\x1b]2;safe title\x07\x1b[23;2t");
            assert_eq!(parser.screen().contents(), contents);
            assert_eq!(parser.screen().cursor_position(), (2, 4));
        }
    }
}

#[test]
fn interleaved_approval_repeats_are_deduplicated_until_a_new_query() {
    let mut app = app(TerminalNotificationMethod::Bell);
    app.terminal_focused = false;
    let mut feedback = TerminalFeedback::new(TitleMode::Disabled, TerminalTarget::Direct);
    let mut bytes = Vec::new();
    for id in ["first", "second", "first", "second"] {
        approval(&mut app, id);
        feedback.update(&mut app, &mut bytes).unwrap();
    }
    assert_eq!(bytes, b"\x07\x07");
    app.begin_terminal_query();
    approval(&mut app, "first");
    feedback.update(&mut app, &mut bytes).unwrap();
    assert_eq!(bytes, b"\x07\x07\x07");
}

#[test]
fn title_components_keep_identity_after_long_paths_and_unterminated_escapes() {
    let mut app = app(TerminalNotificationMethod::Off);
    app.snapshot.cwd = format!("/workspace/{}\x1b]", "p".repeat(255));
    app.set_terminal_thread_title(Some("Parser fix\n\u{2067}ready".into()));
    let title = title_for(&app);
    assert!(title.contains("Parser fix ready"));
    assert!(title.chars().count() <= 240);
    assert!(!title.chars().any(char::is_control));
    assert!(!title.chars().any(|ch| bidi_annotation(ch).is_some()));
}
