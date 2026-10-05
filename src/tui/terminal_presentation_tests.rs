use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers};
use rara_terminal_detection::ColorLevel;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
};

use super::*;
use crate::tui::{
    custom_terminal::Terminal,
    message_role::MessageRole,
    selection::ScreenPosition,
    state::{HelpTab, ListPickerKind, Overlay, StatusTab, TranscriptEntry, TranscriptTurn},
    testing::{TuiHarness, terminal_emulator::EmulatorBackend},
    text_wrap::display_width,
};

fn assert_color(level: ColorLevel, color: Color) {
    match level {
        ColorLevel::Monochrome => assert_eq!(color, Color::Reset),
        ColorLevel::Ansi8 => assert!(
            matches!(
                color,
                Color::Reset
                    | Color::Black
                    | Color::Red
                    | Color::Green
                    | Color::Yellow
                    | Color::Blue
                    | Color::Magenta
                    | Color::Cyan
                    | Color::Gray
            ),
            "{color:?}"
        ),
        ColorLevel::Ansi16 => assert!(
            !matches!(color, Color::Rgb(..) | Color::Indexed(_)),
            "{color:?}"
        ),
        ColorLevel::Ansi256 => assert!(!matches!(color, Color::Rgb(..)), "{color:?}"),
        ColorLevel::TrueColor => {}
    }
}

fn assert_profile(buffer: &Buffer, capabilities: TerminalCapabilities) {
    assert!(
        buffer
            .content
            .iter()
            .any(|cell| cell.symbol().chars().any(char::is_alphanumeric))
    );
    for cell in &buffer.content {
        for color in [cell.fg, cell.bg, cell.underline_color] {
            assert_color(capabilities.colors, color);
        }
        if capabilities.glyphs == GlyphSet::Ascii {
            assert!(
                cell.symbol().is_ascii(),
                "{capabilities:?}: {:?}",
                cell.symbol()
            );
        }
    }
}

#[test]
fn terminal_profiles_cover_complete_buffers_and_cached_surfaces() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let mut startup = TuiHarness::new(Default::default()).unwrap();
    tui.app_mut().restore_committed_turns(vec![TranscriptTurn {
        thinking_duration: None,
        entries: vec![TranscriptEntry::new(MessageRole::Agent,
            "# Heading\n\n**strong** text\n\n```rust\nfn main() { let value = 42; }\n```\n\n```diff\n- old\n+ new\n```\n\nSymbols: \u{25b8} \u{25cf} \u{2713} \u{2610} \u{2026} \u{2514} \u{754c}\n")],
    }]);
    tui.app_mut().sidebar_visible = true;
    let (rich, _) = tui.screen_buffer(120, 48);
    assert!(
        rich.content
            .iter()
            .any(|cell| matches!(cell.fg, Color::Rgb(..)))
    );
    assert!(rich.content.iter().any(|cell| !cell.symbol().is_ascii()));
    let before = tui.app().committed_turns[0].entries[0].message.clone();
    for colors in [
        ColorLevel::Monochrome,
        ColorLevel::Ansi8,
        ColorLevel::Ansi16,
        ColorLevel::Ansi256,
        ColorLevel::TrueColor,
    ] {
        for glyphs in [GlyphSet::Ascii, GlyphSet::Unicode] {
            let capabilities = TerminalCapabilities { colors, glyphs };
            tui.app_mut().terminal_capabilities = capabilities;
            startup.app_mut().terminal_capabilities = capabilities;
            for (width, height) in [(120, 48), (40, 16)] {
                assert_profile(&startup.screen_buffer(width, height).0, capabilities);
            }
            for overlay in [
                None,
                Some(Overlay::Help(HelpTab::General)),
                Some(Overlay::Help(HelpTab::Commands)),
                Some(Overlay::Help(HelpTab::Runtime)),
                Some(Overlay::Status(StatusTab::Overview)),
                Some(Overlay::Status(StatusTab::Config)),
                Some(Overlay::Status(StatusTab::Context)),
                Some(Overlay::Context),
                Some(Overlay::ListPicker(ListPickerKind::Provider)),
                Some(Overlay::ListPicker(ListPickerKind::Resume)),
                Some(Overlay::PermissionPicker),
            ] {
                tui.app_mut().overlay = overlay;
                for (width, height) in [(120, 48), (40, 16)] {
                    let (buffer, _) = tui.screen_buffer(width, height);
                    assert_profile(&buffer, capabilities);
                }
            }
        }
    }
    assert_eq!(tui.app().committed_turns[0].entries[0].message, before);
    tui.app_mut().overlay = None;
    tui.app_mut().terminal_capabilities = TerminalCapabilities::FULL;
    let (restored, _) = tui.screen_buffer(120, 48);
    assert_eq!(restored, rich, "projection must not mutate retained rows");
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "The theme override test asserts the requested RGB value before projection."
)]
fn configured_theme_colors_obey_the_terminal_profile() {
    // Theme configuration is process-wide; keep this fixture out of parallel apps.
    if std::env::var_os("RARA_CAPABILITY_THEME_TEST").is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--nocapture"])
            .env("RARA_CAPABILITY_THEME_TEST", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        return;
    }
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let mut config = crate::config::TuiThemeConfig::default();
    config.tokens.insert("text.accent".into(), "#112233".into());
    config
        .tokens
        .insert("ui.element.bg".into(), "ansi:99".into());
    crate::tui::theme::install_config(&config);
    assert_eq!(
        crate::tui::theme::theme_color(crate::tui::theme::ThemeToken::TextAccent),
        Color::Rgb(17, 34, 51)
    );
    tui.app_mut()
        .open_overlay(Overlay::ListPicker(ListPickerKind::Provider));
    for colors in [
        ColorLevel::Monochrome,
        ColorLevel::Ansi8,
        ColorLevel::Ansi16,
    ] {
        let capabilities = TerminalCapabilities {
            colors,
            glyphs: GlyphSet::Ascii,
        };
        tui.app_mut().terminal_capabilities = capabilities;
        assert_profile(&tui.screen_buffer(80, 24).0, capabilities);
    }
}

#[tokio::test]
async fn ascii_rendering_preserves_input_editing_cursor_and_selection_source() {
    let mut tui = TuiHarness::new(Default::default()).unwrap();
    let input = "ab\u{754c}e\u{301}\u{1f469}\u{200d}\u{1f4bb}z";
    tui.send_terminal_event(Event::Paste(input.into()))
        .await
        .unwrap();
    let (_, rich_cursor) = tui.screen_buffer(80, 24);
    tui.app_mut().terminal_capabilities = TerminalCapabilities {
        colors: ColorLevel::Monochrome,
        glyphs: GlyphSet::Ascii,
    };
    let (ascii, ascii_cursor) = tui.screen_buffer(80, 24);
    assert_eq!(ascii_cursor, rich_cursor);
    assert!(ascii.content.iter().all(|cell| cell.symbol().is_ascii()));
    assert_eq!(tui.app().bottom_pane.input, input);
    for key in [KeyCode::Left, KeyCode::Backspace] {
        tui.press_key(KeyEvent::new(key, KeyModifiers::NONE))
            .await
            .unwrap();
    }
    assert_eq!(tui.app().bottom_pane.input, "ab\u{754c}e\u{301}z");
    let (ascii, cursor) = tui.screen_buffer(80, 24);
    let (x, y) = cursor.unwrap();
    assert_eq!(ascii[(x, y)].symbol(), "z");

    let source = "copy:\u{754c}e\u{301}\u{1f469}\u{200d}\u{1f4bb}:end";
    tui.app_mut().push_entry(MessageRole::User, source);
    let (ascii, _) = tui.screen_buffer(80, 24);
    let (x, y) = (0..24)
        .flat_map(|y| (0..75).map(move |x| (x, y)))
        .find(|&(x, y)| {
            "copy:"
                .chars()
                .enumerate()
                .all(|(i, ch)| ascii[(x + i as u16, y)].symbol() == ch.to_string())
        })
        .expect("visible source prefix");
    let selection = &mut tui.app_mut().transcript_selection;
    assert!(selection.start(ScreenPosition::new(x, y)));
    assert_eq!(
        selection
            .finish(ScreenPosition::new(x + display_width(source) as u16, y))
            .as_deref(),
        Some(source)
    );
}

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "The terminal output oracle must exercise arbitrary RGB and indexed input colors."
)]
fn terminal_writes_use_basic_ansi_and_clear_replaced_wide_ascii_cells() {
    for colors in [
        ColorLevel::Monochrome,
        ColorLevel::Ansi8,
        ColorLevel::Ansi16,
        ColorLevel::Ansi256,
    ] {
        let backend = EmulatorBackend::new(4, 20);
        let screen = backend.screen.clone();
        let mut terminal = Terminal::new(backend).unwrap();
        terminal.set_viewport_area(Rect::new(0, 0, 20, 4));
        for (source, expected, cursor, intensity) in [
            ("A\u{754c}Z", "A??Z", 4, Modifier::BOLD),
            ("AQZ", "AQZ", 3, Modifier::DIM),
        ] {
            terminal
                .draw(|frame| {
                    let line = Line::from(Span::styled(
                        source,
                        Style::default()
                            .fg(Color::Rgb(255, 0, 0))
                            .bg(Color::Indexed(4))
                            .add_modifier(intensity | Modifier::REVERSED),
                    ));
                    frame.render_widget(Paragraph::new(line), frame.area());
                    project_frame(
                        frame.buffer_mut(),
                        TerminalCapabilities {
                            colors,
                            glyphs: GlyphSet::Ascii,
                        },
                    );
                    frame.set_cursor_position((cursor, 0));
                })
                .unwrap();
            let screen = screen.borrow();
            assert_eq!(screen.parser.screen().contents().trim_end(), expected);
            assert_eq!(screen.parser.screen().cursor_position(), (0, cursor));
            let first = screen.parser.screen().cell(0, 0).unwrap();
            assert_eq!(first.bold(), intensity == Modifier::BOLD);
            assert_eq!(first.dim(), intensity == Modifier::DIM);
            assert!(first.inverse());
            if colors == ColorLevel::Monochrome {
                assert_eq!(first.fgcolor(), vt100::Color::Default);
                assert_eq!(first.bgcolor(), vt100::Color::Default);
            } else {
                let red = match colors {
                    ColorLevel::Ansi8 => 1,
                    ColorLevel::Ansi16 => 9,
                    ColorLevel::Ansi256 => 196,
                    ColorLevel::TrueColor | ColorLevel::Monochrome => {
                        unreachable!("limited color fixture")
                    }
                };
                assert_eq!(first.fgcolor(), vt100::Color::Idx(red));
                assert_eq!(first.bgcolor(), vt100::Color::Idx(4));
            }
            assert!(screen.output.is_ascii());
            let output = std::str::from_utf8(&screen.output).unwrap();
            assert!(!output.contains("[38;2;") && !output.contains("[48;2;"));
            if colors != ColorLevel::Ansi256 {
                assert!(!output.contains("[38;5;") && !output.contains("[48;5;"));
            }
        }
    }
}
