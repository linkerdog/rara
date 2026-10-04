use std::io::Write;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::super::Terminal;
use crate::tui::testing::terminal_emulator::EmulatorBackend;

#[test]
fn wide_cell_replacement_preserves_text_and_clears_stale_attributes() {
    let backend = EmulatorBackend::new(3, 16);
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    for text in [
        "old trailing",
        "ab\u{4e2d}tail",
        "abxtail",
        "\u{4e2d}\u{6587}",
        "x",
        "",
    ] {
        terminal
            .draw_inline(|frame| {
                let line = Line::from(vec![
                    Span::styled(text, Style::default().add_modifier(Modifier::BOLD)),
                    Span::raw("!"),
                ]);
                frame.render_widget(Paragraph::new(line), frame.area());
            })
            .unwrap();
        let screen = screen.borrow();
        let expected = format!("{text}!");
        assert_eq!(screen.parser.screen().contents().trim_end(), expected);
        let suffix_column = unicode_width::UnicodeWidthStr::width(text) as u16;
        assert!(
            !screen
                .parser
                .screen()
                .cell(0, suffix_column)
                .unwrap()
                .bold()
        );
        if !text.is_empty() {
            assert!(screen.parser.screen().cell(0, 0).unwrap().bold());
        }
    }
}

#[test]
fn first_inline_frame_preserves_shell_output_in_scrollback() {
    let mut backend = EmulatorBackend::new(8, 24);
    backend.write_all(b"SHELL-ONE\r\nSHELL-TWO\r\n").unwrap();
    let screen = backend.screen.clone();
    screen.borrow_mut().output.clear();
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw_inline(|frame| {
            assert_eq!(frame.area(), Rect::new(0, 0, 24, 8));
            frame.render_widget(Paragraph::new("UI"), frame.area());
        })
        .unwrap();
    let mut screen = screen.borrow_mut();
    assert_eq!(screen.parser.screen().contents().trim_end(), "UI");
    assert!(screen.output.starts_with(b"\x1b[?2026h"));
    assert!(screen.output.ends_with(b"\x1b[?2026l"));
    let bytes = String::from_utf8_lossy(&screen.output);
    assert!(!bytes.contains("\x1b[2J") && !bytes.contains("\x1b[3J"));
    assert!(!screen.output.contains(&0));
    screen.parser.screen_mut().set_scrollback(1000);
    let history = screen.parser.screen().contents();
    assert!(history.contains("SHELL-ONE"), "{history}");
    assert!(history.contains("SHELL-TWO"), "{history}");
}

#[test]
fn resize_repaints_blank_first_columns_without_erasing_screen() {
    let backend = EmulatorBackend::new(5, 24);
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw_inline(|frame| {
            frame.render_widget(Paragraph::new("AAAA\nBBBB\nCCCC\nDDDD"), frame.area());
        })
        .unwrap();
    for (rows, columns) in [(8, 30), (3, 12), (1, 1)] {
        {
            let mut screen = screen.borrow_mut();
            screen.parser.screen_mut().set_size(rows, columns);
            screen.parser.process(b"\x1b[HX");
            screen.output.clear();
        }
        terminal.draw_inline(|_| {}).unwrap();
        let screen = screen.borrow();
        assert!(screen.parser.screen().contents().trim().is_empty());
        assert_eq!(terminal.viewport_area, Rect::new(0, 0, columns, rows));
        assert!(!String::from_utf8_lossy(&screen.output).contains("\x1b[2J"));
        assert!(!screen.output.contains(&0));
    }
}

#[test]
fn startup_preserves_shell_history_without_querying_cursor() {
    let mut backend = EmulatorBackend::new(5, 30);
    backend
        .write_all(b"FIRST\r\nSECOND\r\nCURRENT-SHELL-LINE")
        .unwrap();
    backend.fail_cursor_query = true;
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    terminal.draw_inline(|_| {}).unwrap();
    assert_eq!(terminal.backend.cursor_queries, 0);
    let mut screen = screen.borrow_mut();
    screen.parser.screen_mut().set_scrollback(1000);
    let history = screen.parser.screen().contents();
    for line in ["FIRST", "SECOND", "CURRENT-SHELL-LINE"] {
        assert!(history.contains(line), "missing {line}: {history}");
    }
}

#[test]
fn resize_burst_repaints_even_when_final_dimensions_match() {
    let backend = EmulatorBackend::new(4, 24);
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    terminal
        .draw_inline(|frame| {
            frame.render_widget(Paragraph::new("STABLE"), frame.area());
        })
        .unwrap();
    screen
        .borrow_mut()
        .parser
        .process(b"\x1b[HSTALE\r\nLEFTOVER");
    terminal.invalidate_viewport();
    terminal
        .draw_inline(|frame| {
            frame.render_widget(Paragraph::new("STABLE"), frame.area());
        })
        .unwrap();
    assert_eq!(
        screen.borrow().parser.screen().contents().trim_end(),
        "STABLE"
    );
}

#[test]
fn shell_handoff_starts_a_clean_line_once_and_can_reserve_again() {
    let backend = EmulatorBackend::new(4, 24);
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    for _ in 0..2 {
        terminal
            .draw_inline(|frame| {
                frame.render_widget(Paragraph::new("ONE\nTWO\nTHREE\nFOOTER"), frame.area());
            })
            .unwrap();
        assert!(
            screen
                .borrow()
                .parser
                .screen()
                .contents()
                .starts_with("ONE")
        );
        terminal.finish_inline_viewport().unwrap();
        {
            let screen = screen.borrow();
            assert_eq!(screen.parser.screen().cursor_position(), (3, 0));
            assert!(screen.parser.screen().contents().contains("FOOTER"));
            assert_eq!(screen.parser.screen().cell(3, 0).unwrap().contents(), "");
        }
        let output_size = screen.borrow().output.len();
        terminal.finish_inline_viewport().unwrap();
        assert_eq!(screen.borrow().output.len(), output_size);
    }
}

#[test]
fn shell_handoff_after_an_unpainted_resize_uses_the_current_bottom_row() {
    for rows in [8, 2] {
        let backend = EmulatorBackend::new(4, 24);
        let screen = backend.screen.clone();
        let mut terminal = Terminal::new(backend).unwrap();
        terminal
            .draw_inline(|frame| {
                frame.render_widget(Paragraph::new("ONE\nTWO\nTHREE\nFOOTER"), frame.area());
            })
            .unwrap();
        screen.borrow_mut().parser.screen_mut().set_size(rows, 24);
        terminal.finish_inline_viewport().unwrap();
        assert_eq!(
            screen.borrow().parser.screen().cursor_position(),
            (rows - 1, 0)
        );
    }
}

#[test]
fn failed_frame_ends_synchronized_output_and_keeps_original_error() {
    let backend = EmulatorBackend::new(4, 20);
    let screen = backend.screen.clone();
    let mut terminal = Terminal::new(backend).unwrap();
    let error = terminal
        .draw_inline(|frame| {
            frame.render_widget(Paragraph::new("FRAME"), frame.area());
            screen.borrow_mut().fail_next_write = true;
        })
        .unwrap_err();
    assert_eq!(error.to_string(), "injected terminal write failure");
    assert!(screen.borrow().output.ends_with(b"\x1b[?2026l"));
    terminal.finish_inline_viewport().unwrap();
    assert_eq!(screen.borrow().parser.screen().cursor_position(), (3, 0));
}
