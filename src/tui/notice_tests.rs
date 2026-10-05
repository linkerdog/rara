use std::sync::Arc;

use super::app_event::AppEvent;
use super::event_dispatch::dispatch_event;
use super::message_role::MessageRole;
use super::state::TuiApp;
use crate::config::ConfigManager;
use crate::oauth::OAuthManager;

async fn save_base_url() -> (tempfile::TempDir, TuiApp) {
    let temp = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    app.base_url_input = "https://user:secret-password@example.com/v1?token=secret-query".into();
    let oauth = Arc::new(OAuthManager::new_for_config_dir(temp.path().join("oauth")).unwrap());
    dispatch_event(AppEvent::SaveBaseUrlInput, &mut app, &mut None, &oauth)
        .await
        .unwrap();
    (temp, app)
}

#[tokio::test]
async fn settings_notice_redacts_secrets_before_display() {
    let (_temp, app) = save_base_url().await;
    let notice = app.notice_text().unwrap();
    for secret in ["secret-password", "secret-query", "user:"] {
        assert!(!notice.contains(secret), "notice contains {secret}");
    }
    assert!(notice.contains("example.com"));
}

#[tokio::test]
async fn settings_notice_records_one_redacted_transcript_entry() {
    let (_temp, app) = save_base_url().await;
    let entries = app
        .active_turn
        .entries
        .iter()
        .filter(|entry| {
            entry.role == MessageRole::System && entry.message.starts_with("Saved base URL:")
        })
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert!(!entries[0].message.contains("secret-password"));
    assert!(!entries[0].message.contains("secret-query"));
}

#[test]
fn status_buffer_uses_typed_severity_and_returns_to_ready_after_expiry() {
    use ratatui::{buffer::Buffer, layout::Rect};

    use crate::tui::custom_terminal::Frame;
    use crate::tui::state::NoticeLevel;
    use crate::tui::theme::{STATUS_ERROR, STATUS_READY, STATUS_WARNING};

    let dir = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: dir.path().join("config.json"),
    })
    .unwrap();
    for (level, label, color, message) in [
        (
            NoticeLevel::Info,
            "Ready",
            STATUS_READY,
            "Warning: informational text",
        ),
        (
            NoticeLevel::Warning,
            "Warning",
            STATUS_WARNING,
            "Provider needs attention",
        ),
        (
            NoticeLevel::Error,
            "Error",
            STATUS_ERROR,
            "Provider request rejected",
        ),
    ] {
        app.push_notice(level, message);
        for expired in [false, true] {
            if expired {
                assert!(app.expire_notice(
                    tokio::time::Instant::now() + std::time::Duration::from_secs(8)
                ));
            }
            let (label, color, detail) = if expired {
                ("Ready", STATUS_READY, "waiting for input")
            } else {
                (label, color, message)
            };
            let area = Rect::new(0, 0, 100, 24);
            let mut buffer = Buffer::empty(area);
            let mut frame = Frame {
                cursor_position: None,
                viewport_area: area,
                buffer: &mut buffer,
            };
            crate::tui::render::render(&mut frame, &mut app);
            let y = (0..area.height)
                .find(|y| {
                    let row = (0..area.width)
                        .map(|x| buffer[(x, *y)].symbol())
                        .collect::<String>();
                    row.contains(label) && row.contains(detail)
                })
                .expect("typed status row");
            let x = (0..area.width - label.len() as u16)
                .find(|x| {
                    (0..label.len() as u16)
                        .map(|dx| buffer[(*x + dx, y)].symbol())
                        .collect::<String>()
                        == label
                })
                .expect("status label");
            for dx in 0..label.len() as u16 {
                assert_eq!(buffer[(x + dx, y)].fg, color);
            }
            assert!(
                app.active_turn
                    .entries
                    .iter()
                    .any(|entry| entry.message == message)
            );
        }
    }
}
