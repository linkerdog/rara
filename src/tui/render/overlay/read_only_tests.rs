use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use ratatui::{buffer::Buffer, widgets::Widget};

use super::*;
use crate::tui::message_role::MessageRole;
use crate::tui::testing::TuiHarness;
use crate::tui::text_wrap::display_width;

const OVERLAYS: [Overlay; 7] = [
    Overlay::Help(HelpTab::General),
    Overlay::Help(HelpTab::Commands),
    Overlay::Help(HelpTab::Runtime),
    Overlay::Status(StatusTab::Overview),
    Overlay::Status(StatusTab::Config),
    Overlay::Status(StatusTab::Context),
    Overlay::Context,
];

fn harness(overlay: Overlay) -> TuiHarness {
    let mut harness = TuiHarness::new(Default::default()).expect("isolated harness");
    let long_path = format!("/workspace/{}/PATH-END", "\u{8def}\u{5f84}-".repeat(60));
    let app = harness.app_mut();
    app.snapshot.cwd = long_path.clone();
    app.snapshot.planning_lifecycle.plan_path = Some(long_path);
    app.config.base_url = Some(format!(
        "https://example.test/{}/URL-END",
        "segment/".repeat(60)
    ));
    app.push_entry(
        MessageRole::Agent,
        format!("{}RECENT-END", "\u{8def}\u{5f84}".repeat(30)),
    );
    app.open_overlay(overlay);
    harness
}

async fn press(harness: &mut TuiHarness, code: KeyCode) {
    assert!(
        !harness
            .press_key(KeyEvent::new(code, KeyModifiers::NONE))
            .await
            .expect("key dispatch")
    );
}

async fn wheel(harness: &mut TuiHarness, kind: MouseEventKind) {
    harness
        .send_terminal_event(Event::Mouse(MouseEvent {
            kind,
            column: 4,
            row: 4,
            modifiers: KeyModifiers::NONE,
        }))
        .await
        .expect("wheel dispatch");
}

fn body_area(harness: &TuiHarness, width: u16, height: u16) -> Rect {
    let popup = super::super::popup_rect(Rect::new(0, 0, width, height), 80, 60);
    let inner = popup_block().inner(popup);
    let layout = harness
        .app()
        .overlay_scroll
        .layout()
        .expect("measured body");
    Rect::new(inner.x, inner.y + 1, layout.width, layout.height)
}

fn assert_visible_rows(harness: &mut TuiHarness, width: u16, height: u16) {
    let (actual, _) = harness.screen_buffer(width, height);
    let area = body_area(harness, width, height);
    let app = harness.app();
    let body = OverlayBody::new(app, app.overlay.expect("overlay"), area.width).expect("body");
    assert!(
        body.rows
            .iter()
            .all(|line| display_width(&line.to_string()) <= usize::from(area.width))
    );
    let mut expected = Buffer::empty(area);
    Paragraph::new(body.rows[app.overlay_scroll.visible_range()].to_vec())
        .render(area, &mut expected);
    for y in area.y..area.bottom() {
        for x in area.x..area.right() {
            assert_eq!(
                actual[(x, y)].symbol(),
                expected[(x, y)].symbol(),
                "cell ({x}, {y})"
            );
        }
    }
}

#[tokio::test]
async fn all_tabs_wrap_and_reach_their_final_rows_in_the_short_terminal_matrix() {
    for (width, height) in [(80, 24), (60, 20), (40, 12)] {
        for overlay in OVERLAYS {
            let mut harness = harness(overlay);
            assert_visible_rows(&mut harness, width, height);
            let layout = harness.app().overlay_scroll.layout().expect("body");
            assert!(layout.height > 0);
            assert!(
                layout.content_rows > usize::from(layout.height),
                "scrollable {overlay:?}"
            );
            press(&mut harness, KeyCode::End).await;
            assert_visible_rows(&mut harness, width, height);
            assert_eq!(
                harness.app().overlay_scroll.visible_range().end,
                layout.content_rows,
                "{overlay:?} at {width}x{height}"
            );
            let before = harness.app().overlay_scroll.offset();
            press(&mut harness, KeyCode::PageUp).await;
            assert!(harness.app().overlay_scroll.offset() < before);
            press(&mut harness, KeyCode::Home).await;
            assert_eq!(harness.app().overlay_scroll.offset(), 0);
            press(&mut harness, KeyCode::PageDown).await;
            assert_eq!(
                harness.app().overlay_scroll.offset(),
                usize::from(layout.height - 1)
                    .min(layout.content_rows - usize::from(layout.height))
            );
            assert_visible_rows(&mut harness, width, height);
            harness.expect_no_commands();
        }
    }
}

#[tokio::test]
async fn repeated_boundary_input_cannot_create_scroll_debt_or_edit_the_composer() {
    for overlay in OVERLAYS
        .into_iter()
        .filter(|overlay| *overlay != Overlay::Help(HelpTab::Commands))
    {
        let mut harness = harness(overlay);
        harness.app_mut().bottom_pane.input = "preserved draft".into();
        harness.screen_buffer(40, 12);
        let transcript = harness.app().transcript_scroll;
        press(&mut harness, KeyCode::End).await;
        let end = harness.app().overlay_scroll.offset();
        for _ in 0..40 {
            press(&mut harness, KeyCode::Down).await;
            wheel(&mut harness, MouseEventKind::ScrollDown).await;
            assert_eq!(harness.app().overlay_scroll.offset(), end);
        }
        press(&mut harness, KeyCode::Up).await;
        assert_eq!(harness.app().overlay_scroll.offset(), end - 1);
        press(&mut harness, KeyCode::Home).await;
        for _ in 0..40 {
            press(&mut harness, KeyCode::Char('k')).await;
            wheel(&mut harness, MouseEventKind::ScrollUp).await;
        }
        press(&mut harness, KeyCode::Char('j')).await;
        assert_eq!(harness.app().overlay_scroll.offset(), 1);
        wheel(&mut harness, MouseEventKind::ScrollDown).await;
        assert!(harness.app().overlay_scroll.offset() > 1);
        assert_eq!(harness.app().bottom_pane.input, "preserved draft");
        assert_eq!(harness.app().transcript_scroll, transcript);
    }
}

#[tokio::test]
async fn command_navigation_keeps_wrapped_entries_visible_and_pages_to_the_last_description() {
    let mut harness = harness(Overlay::Help(HelpTab::Commands));
    harness.screen_buffer(40, 12);
    let count = help_command_items("").len();
    for _ in 0..count {
        press(&mut harness, KeyCode::Down).await;
        assert_visible_rows(&mut harness, 40, 12);
        let body =
            OverlayBody::new(harness.app(), Overlay::Help(HelpTab::Commands), 34).expect("body");
        let selected = &body.commands[harness.app().command_palette_idx];
        assert!(
            harness
                .app()
                .overlay_scroll
                .visible_range()
                .contains(&selected.start)
        );
    }
    assert_eq!(harness.app().command_palette_idx, count - 1);
    press(&mut harness, KeyCode::End).await;
    assert_visible_rows(&mut harness, 40, 12);
    let layout = harness.app().overlay_scroll.layout().expect("layout");
    assert_eq!(
        harness.app().overlay_scroll.visible_range().end,
        layout.content_rows
    );
    press(&mut harness, KeyCode::Home).await;
    assert_eq!(harness.app().command_palette_idx, 0);
    wheel(&mut harness, MouseEventKind::ScrollDown).await;
    assert!(harness.app().command_palette_idx > 0);
    harness
        .app_mut()
        .open_overlay(Overlay::Help(HelpTab::Commands));
    assert_eq!(harness.app().command_palette_idx, 0);
    assert_visible_rows(&mut harness, 40, 12);
    assert_eq!(harness.app().overlay_scroll.offset(), 0);
}

#[tokio::test]
async fn content_shrink_is_clamped_on_input_before_the_next_frame() {
    let mut harness = harness(Overlay::Status(StatusTab::Overview));
    harness.app_mut().snapshot.cwd = "long-path/".repeat(1000);
    harness.screen_buffer(40, 12);
    press(&mut harness, KeyCode::End).await;
    let old_end = harness.app().overlay_scroll.offset();
    harness.app_mut().snapshot.cwd = "/short".into();
    press(&mut harness, KeyCode::Up).await;
    let layout = harness
        .app()
        .overlay_scroll
        .layout()
        .expect("refreshed content");
    let end = layout.content_rows - usize::from(layout.height);
    assert!(end < old_end);
    assert_eq!(harness.app().overlay_scroll.offset(), end - 1);
    assert_visible_rows(&mut harness, 40, 12);
    press(&mut harness, KeyCode::End).await;
    assert_visible_rows(&mut harness, 80, 24);
    let resized = harness.app().overlay_scroll.layout().expect("resized");
    assert!(
        harness.app().overlay_scroll.offset()
            <= resized
                .content_rows
                .saturating_sub(usize::from(resized.height))
    );
}

#[tokio::test]
async fn switching_tabs_and_reopening_resets_the_body_without_moving_the_draft() {
    let mut harness = harness(Overlay::Status(StatusTab::Overview));
    harness.app_mut().bottom_pane.input = "preserved draft".into();
    harness.screen_buffer(40, 12);
    press(&mut harness, KeyCode::End).await;
    press(&mut harness, KeyCode::Char('2')).await;
    assert_eq!(harness.app().overlay_scroll.offset(), 0);
    assert!(harness.app().overlay_scroll.layout().is_none());
    harness.screen_buffer(40, 12);
    press(&mut harness, KeyCode::End).await;
    harness.app_mut().open_overlay(Overlay::Context);
    assert_eq!(harness.app().overlay_scroll.offset(), 0);
    assert_eq!(harness.app().bottom_pane.input, "preserved draft");
}

#[tokio::test]
async fn scrolling_preserves_complete_paths_and_every_runtime_help_section() {
    for overlay in [
        Overlay::Help(HelpTab::General),
        Overlay::Help(HelpTab::Runtime),
        Overlay::Status(StatusTab::Overview),
        Overlay::Context,
    ] {
        let mut harness = harness(overlay);
        harness.screen_buffer(40, 12);
        let area = body_area(&harness, 40, 12);
        let row_count = harness
            .app()
            .overlay_scroll
            .layout()
            .expect("body")
            .content_rows;
        let mut observed = String::new();
        let mut reached_end = false;
        for _ in 0..=row_count {
            let (buffer, _) = harness.screen_buffer(40, 12);
            let before = harness.app().overlay_scroll.offset();
            press(&mut harness, KeyCode::Down).await;
            let at_end = harness.app().overlay_scroll.offset() == before;
            let bottom = if at_end { area.bottom() } else { area.y + 1 };
            for y in area.y..bottom {
                for x in area.x..area.right() {
                    observed.extend(
                        buffer[(x, y)]
                            .symbol()
                            .chars()
                            .filter(|ch| !ch.is_whitespace()),
                    );
                }
            }
            if at_end {
                reached_end = true;
                break;
            }
        }
        assert!(
            reached_end,
            "navigation must stop at the measured content boundary"
        );
        if overlay == Overlay::Help(HelpTab::General) {
            let expected = general_help_text()
                .chars()
                .filter(|ch| !ch.is_whitespace())
                .collect::<String>();
            assert!(
                observed.contains(&expected),
                "general help must remain complete"
            );
        } else {
            assert!(
                observed.contains(&harness.app().snapshot.cwd),
                "the entire CJK path must be rendered: {overlay:?}"
            );
        }
        if overlay == Overlay::Help(HelpTab::Runtime) {
            for section in [
                "runtime",
                "workspace",
                "promptsources",
                "metrics",
                "models/recent",
                "RECENT-END",
            ] {
                assert!(observed.contains(section), "missing {section}");
            }
        }
    }
}

#[test]
fn tiny_overlay_geometry_remains_bounded() {
    for overlay in OVERLAYS {
        for (width, height) in [(0, 0), (1, 1), (2, 2), (12, 4)] {
            let mut harness = harness(overlay);
            harness.screen_buffer(width, height);
            assert_eq!(harness.app().overlay_scroll.offset(), 0);
        }
    }
}
