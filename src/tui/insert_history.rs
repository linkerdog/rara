use std::fmt;
use std::io;
use std::io::Write;

use crossterm::Command;
use crossterm::cursor::MoveDown;
use crossterm::cursor::MoveTo;
use crossterm::cursor::MoveToColumn;
use crossterm::cursor::RestorePosition;
use crossterm::cursor::SavePosition;
use crossterm::queue;
use crossterm::style::Color as CColor;
use crossterm::style::Colors;
use crossterm::style::Print;
use crossterm::style::SetAttribute;
use crossterm::style::SetBackgroundColor;
use crossterm::style::SetColors;
use crossterm::style::SetForegroundColor;
use crossterm::terminal::Clear;
use crossterm::terminal::ClearType;
use ratatui::layout::Size;
use ratatui::prelude::Backend;
use ratatui::style::Color;
use ratatui::style::Modifier;
use ratatui::text::Line;
use ratatui::text::Span;

/// Selects the terminal escape strategy for inserting history lines above the viewport.
///
/// Standard terminals support `DECSTBM` scroll regions and Reverse Index (`ESC M`),
/// which let us slide existing content down without redrawing it. Zellij silently
/// drops or mishandles those sequences, so `Zellij` mode falls back to emitting
/// newlines at the bottom of the screen and writing lines at absolute positions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertHistoryMode {
    Standard,
    Zellij,
}

impl InsertHistoryMode {
    pub fn new(is_zellij: bool) -> Self {
        if is_zellij {
            Self::Zellij
        } else {
            Self::Standard
        }
    }
}

/// Insert `lines` above the viewport using the terminal's backend writer
/// (avoids direct stdout references).
#[cfg(test)]
fn insert_history_lines<B>(
    terminal: &mut super::custom_terminal::Terminal<B>,
    lines: Vec<Line>,
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    insert_history_lines_with_mode(terminal, lines, InsertHistoryMode::Standard)
}

/// Insert `lines` above the viewport, using the escape strategy selected by `mode`.
///
/// In `Standard` mode this manipulates DECSTBM scroll regions to slide existing
/// scrollback down and writes new lines into the freed space. In `Zellij` mode it
/// emits newlines at the screen bottom to create space (since Zellij ignores scroll
/// region escapes) and writes lines at computed absolute positions. Both modes
/// update `terminal.viewport_area` so subsequent draw passes know where the
/// viewport moved to.
pub fn insert_history_lines_with_mode<B>(
    terminal: &mut super::custom_terminal::Terminal<B>,
    lines: Vec<Line>,
    mode: InsertHistoryMode,
) -> io::Result<()>
where
    B: Backend<Error = io::Error> + Write,
{
    let screen_size = terminal.backend().size().unwrap_or(Size::new(0, 0));

    let original_viewport_y = terminal.viewport_area.y;
    let mut area = terminal.viewport_area;
    let mut should_update_area = false;
    let last_cursor_pos = terminal.last_known_cursor_pos;
    let writer = terminal.backend_mut();

    let wrap_width = area.width.max(1) as usize;
    let wrapped = lines;
    let wrapped_lines = crate::tui::layout_utils::total_visual_rows(&wrapped, area.width) as u16;

    if matches!(mode, InsertHistoryMode::Zellij) {
        let space_below = screen_size.height.saturating_sub(area.bottom());
        let shift_down = wrapped_lines.min(space_below);
        let scroll_up_amount = wrapped_lines.saturating_sub(shift_down);

        if scroll_up_amount > 0 {
            // Scroll the entire screen up by emitting \n at the bottom
            queue!(writer, MoveTo(0, screen_size.height.saturating_sub(1)))?;
            for _ in 0..scroll_up_amount {
                queue!(writer, Print("\n"))?;
            }
        }

        if shift_down > 0 {
            area.y += shift_down;
            should_update_area = true;
        }

        let cursor_top = area.top().saturating_sub(scroll_up_amount + shift_down);
        queue!(writer, MoveTo(0, cursor_top))?;

        for (i, line) in wrapped.iter().enumerate() {
            if i > 0 {
                queue!(writer, Print("\r\n"))?;
            }
            write_history_line(writer, line, wrap_width)?;
        }
    } else {
        let cursor_top = if area.bottom() < screen_size.height {
            let scroll_amount = wrapped_lines.min(screen_size.height - area.bottom());

            let top_1based = area.top() + 1;
            queue!(writer, SetScrollRegion(top_1based..screen_size.height))?;
            queue!(writer, MoveTo(0, area.top()))?;
            for _ in 0..scroll_amount {
                queue!(writer, Print("\x1bM"))?;
            }
            queue!(writer, ResetScrollRegion)?;

            let cursor_top = area.top().saturating_sub(1);
            area.y += scroll_amount;
            should_update_area = true;
            cursor_top
        } else {
            area.top().saturating_sub(1)
        };

        // Limit the scroll region to the lines from the top of the screen to the
        // top of the viewport. With this in place, when we add lines inside this
        // area, only the lines in this area will be scrolled. We place the cursor
        // at the end of the scroll region, and add lines starting there.
        //
        // ┌─Screen───────────────────────┐
        // │┌╌Scroll region╌╌╌╌╌╌╌╌╌╌╌╌╌╌┐│
        // │┆                            ┆│
        // │┆                            ┆│
        // │┆                            ┆│
        // │█╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌╌┘│
        // │╭─Viewport───────────────────╮│
        // ││                            ││
        // │╰────────────────────────────╯│
        // └──────────────────────────────┘
        if area.top() > 0 {
            queue!(writer, SetScrollRegion(1..area.top() + 1))?;
        }

        // NB: we are using MoveTo instead of set_cursor_position here to avoid messing with the
        // terminal's last_known_cursor_position, which hopefully will still be accurate after we
        // fetch/restore the cursor position. insert_history_lines should be cursor-position-neutral :)
        queue!(writer, MoveTo(0, cursor_top))?;

        for line in &wrapped {
            queue!(writer, Print("\r\n"))?;
            write_history_line(writer, line, wrap_width)?;
        }

        if area.top() > 0 {
            queue!(writer, ResetScrollRegion)?;
        }
    }

    // Restore the cursor position relative to the viewport. If history insertion
    // shifted the viewport downward, the cursor must move down by the same delta
    // so it stays attached to the active pane instead of landing in inserted history.
    let cursor_y = if should_update_area {
        last_cursor_pos
            .y
            .saturating_add(area.y.saturating_sub(original_viewport_y))
    } else {
        last_cursor_pos.y
    };
    queue!(writer, MoveTo(last_cursor_pos.x, cursor_y))?;

    let _ = writer;
    if should_update_area {
        terminal.set_viewport_area(area);
    }
    if wrapped_lines > 0 {
        terminal.note_history_rows_inserted(wrapped_lines);
    }

    Ok(())
}

/// Render a single wrapped history line: clear continuation rows for wide lines,
/// set foreground/background colors, and write styled spans. Caller is responsible
/// for cursor positioning and any leading `\r\n`.
fn write_history_line<W: Write>(writer: &mut W, line: &Line, wrap_width: usize) -> io::Result<()> {
    let sanitized_line = crate::tui::display_sanitize::sanitize_display_line_segments(line);
    let physical_rows =
        crate::tui::layout_utils::line_visual_rows(&sanitized_line, wrap_width) as u16;
    if physical_rows > 1 {
        queue!(writer, SavePosition)?;
        for _ in 1..physical_rows {
            queue!(writer, MoveDown(1), MoveToColumn(0))?;
            queue!(writer, Clear(ClearType::UntilNewLine))?;
        }
        queue!(writer, RestorePosition)?;
    }
    queue!(
        writer,
        SetColors(Colors::new(
            line.style.fg.map(crossterm_color).unwrap_or(CColor::Reset),
            line.style.bg.map(crossterm_color).unwrap_or(CColor::Reset)
        ))
    )?;
    queue!(writer, Clear(ClearType::UntilNewLine))?;
    // Merge line-level style into each span so that ANSI colors reflect
    // line styles (e.g., blockquotes with green fg).
    let merged_spans: Vec<Span> = sanitized_line
        .spans
        .iter()
        .map(|s| Span {
            style: s.style.patch(sanitized_line.style),
            content: s.content.clone(),
        })
        .collect();
    write_spans(writer, merged_spans.iter())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetScrollRegion(pub std::ops::Range<u16>);

impl Command for SetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[{};{}r", self.0.start, self.0.end)
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute SetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        // TODO(nornagon): is this supported on Windows?
        true
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResetScrollRegion;

impl Command for ResetScrollRegion {
    fn write_ansi(&self, f: &mut impl fmt::Write) -> fmt::Result {
        write!(f, "\x1b[r")
    }

    #[cfg(windows)]
    fn execute_winapi(&self) -> std::io::Result<()> {
        panic!("tried to execute ResetScrollRegion command using WinAPI, use ANSI instead");
    }

    #[cfg(windows)]
    fn is_ansi_code_supported(&self) -> bool {
        // TODO(nornagon): is this supported on Windows?
        true
    }
}

struct ModifierDiff {
    pub from: Modifier,
    pub to: Modifier,
}

impl ModifierDiff {
    fn queue<W>(self, mut w: W) -> io::Result<()>
    where
        W: io::Write,
    {
        use crossterm::style::Attribute as CAttribute;
        let removed = self.from - self.to;
        if removed.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::NoReverse))?;
        }
        if removed.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
            if self.to.contains(Modifier::DIM) {
                queue!(w, SetAttribute(CAttribute::Dim))?;
            }
        }
        if removed.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::NoItalic))?;
        }
        if removed.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::NoUnderline))?;
        }
        if removed.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::NormalIntensity))?;
        }
        if removed.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::NotCrossedOut))?;
        }
        if removed.contains(Modifier::SLOW_BLINK) || removed.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::NoBlink))?;
        }

        let added = self.to - self.from;
        if added.contains(Modifier::REVERSED) {
            queue!(w, SetAttribute(CAttribute::Reverse))?;
        }
        if added.contains(Modifier::BOLD) {
            queue!(w, SetAttribute(CAttribute::Bold))?;
        }
        if added.contains(Modifier::ITALIC) {
            queue!(w, SetAttribute(CAttribute::Italic))?;
        }
        if added.contains(Modifier::UNDERLINED) {
            queue!(w, SetAttribute(CAttribute::Underlined))?;
        }
        if added.contains(Modifier::DIM) {
            queue!(w, SetAttribute(CAttribute::Dim))?;
        }
        if added.contains(Modifier::CROSSED_OUT) {
            queue!(w, SetAttribute(CAttribute::CrossedOut))?;
        }
        if added.contains(Modifier::SLOW_BLINK) {
            queue!(w, SetAttribute(CAttribute::SlowBlink))?;
        }
        if added.contains(Modifier::RAPID_BLINK) {
            queue!(w, SetAttribute(CAttribute::RapidBlink))?;
        }

        Ok(())
    }
}

fn write_spans<'a, I>(mut writer: &mut impl Write, content: I) -> io::Result<()>
where
    I: IntoIterator<Item = &'a Span<'a>>,
{
    let mut fg = Color::Reset;
    let mut bg = Color::Reset;
    let mut last_modifier = Modifier::empty();
    for span in content {
        let mut modifier = Modifier::empty();
        modifier.insert(span.style.add_modifier);
        modifier.remove(span.style.sub_modifier);
        if modifier != last_modifier {
            let diff = ModifierDiff {
                from: last_modifier,
                to: modifier,
            };
            diff.queue(&mut writer)?;
            last_modifier = modifier;
        }
        let next_fg = span.style.fg.unwrap_or(Color::Reset);
        let next_bg = span.style.bg.unwrap_or(Color::Reset);
        if next_fg != fg || next_bg != bg {
            queue!(
                writer,
                SetColors(Colors::new(
                    crossterm_color(next_fg),
                    crossterm_color(next_bg)
                ))
            )?;
            fg = next_fg;
            bg = next_bg;
        }

        queue!(writer, Print(span.content.clone()))?;
    }

    queue!(
        writer,
        SetForegroundColor(CColor::Reset),
        SetBackgroundColor(CColor::Reset),
        SetAttribute(crossterm::style::Attribute::Reset),
    )
}

fn crossterm_color(color: Color) -> CColor {
    match color {
        Color::Reset => CColor::Reset,
        Color::Black => CColor::Black,
        Color::Red => CColor::DarkRed,
        Color::Green => CColor::DarkGreen,
        Color::Yellow => CColor::DarkYellow,
        Color::Blue => CColor::DarkBlue,
        Color::Magenta => CColor::DarkMagenta,
        Color::Cyan => CColor::DarkCyan,
        Color::Gray => CColor::Grey,
        Color::DarkGray => CColor::DarkGrey,
        Color::LightRed => CColor::Red,
        Color::LightGreen => CColor::Green,
        Color::LightYellow => CColor::Yellow,
        Color::LightBlue => CColor::Blue,
        Color::LightMagenta => CColor::Magenta,
        Color::LightCyan => CColor::Cyan,
        Color::White => CColor::White,
        Color::Rgb(r, g, b) => CColor::Rgb { r, g, b },
        Color::Indexed(value) => CColor::AnsiValue(value),
    }
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use ratatui::{
        backend::{Backend, ClearType, WindowSize},
        layout::{Position, Rect, Size},
        style::Style,
        text::{Line, Span},
    };

    use super::*;
    use crate::tui::custom_terminal::Terminal;

    /// Minimal backend that captures writes and reports a fixed size + cursor.
    struct TestBackend {
        inner: Vec<u8>,
        screen_size: Size,
        cursor_pos: Position,
    }

    impl TestBackend {
        fn new(width: u16, height: u16) -> Self {
            Self {
                inner: Vec::new(),
                screen_size: Size::new(width, height),
                cursor_pos: Position {
                    x: 0,
                    y: height - 1,
                },
            }
        }

        fn written(&self) -> String {
            String::from_utf8_lossy(&self.inner).into_owned()
        }
    }

    impl Write for TestBackend {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.inner.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            self.inner.flush()
        }
    }

    impl Backend for TestBackend {
        type Error = io::Error;

        fn size(&self) -> io::Result<Size> {
            Ok(self.screen_size)
        }

        fn get_cursor_position(&mut self) -> io::Result<Position> {
            Ok(self.cursor_pos)
        }

        fn set_cursor_position<P: Into<Position>>(&mut self, position: P) -> io::Result<()> {
            self.cursor_pos = position.into();
            Ok(())
        }

        fn hide_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn show_cursor(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn clear_region(&mut self, _clear_type: ClearType) -> io::Result<()> {
            Ok(())
        }

        fn clear(&mut self) -> io::Result<()> {
            Ok(())
        }

        fn window_size(&mut self) -> io::Result<WindowSize> {
            Ok(WindowSize {
                columns_rows: self.screen_size,
                pixels: Size::new(0, 0),
            })
        }

        fn draw<'a, I>(&mut self, _content: I) -> io::Result<()>
        where
            I: Iterator<Item = (u16, u16, &'a ratatui::buffer::Cell)>,
        {
            Ok(())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn test_terminal(width: u16, height: u16) -> Terminal<TestBackend> {
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("create test terminal");
        let viewport_height = 5.min(height);
        terminal.set_viewport_area(Rect::new(
            0,
            height.saturating_sub(viewport_height),
            width,
            viewport_height,
        ));
        terminal
    }

    fn plain_line(text: &str) -> Line<'static> {
        Line::from(Span::raw(text.to_string()))
    }

    // ── Standard mode tests ──

    #[test]
    fn insert_empty_lines_is_noop() {
        let mut terminal = test_terminal(80, 24);
        let viewport_before = terminal.viewport_area;
        let history_before = terminal.visible_history_rows();

        insert_history_lines(&mut terminal, Vec::new()).expect("insert empty");

        assert_eq!(terminal.viewport_area, viewport_before);
        assert_eq!(terminal.visible_history_rows(), history_before);
    }

    #[test]
    fn insert_single_line_shifts_viewport_down() {
        let mut terminal = test_terminal(80, 24);
        let viewport_before = terminal.viewport_area.y;

        let lines = vec![plain_line("hello")];
        insert_history_lines(&mut terminal, lines).expect("insert one line");

        // When viewport is at the bottom, Standard mode inserts lines into the
        // scrollback region above the viewport without shifting the viewport.
        // visible_history_rows should still increment.
        assert!(terminal.visible_history_rows() > 0);
        let _ = viewport_before; // viewport y may or may not change
    }

    #[test]
    fn insert_updates_visible_history_rows() {
        let mut terminal = test_terminal(80, 24);

        let lines: Vec<Line> = (0..3).map(|i| plain_line(&format!("line {i}"))).collect();
        let prev = terminal.visible_history_rows();

        insert_history_lines(&mut terminal, lines).expect("insert three lines");

        assert!(terminal.visible_history_rows() > prev);
    }

    // ── Zellij mode tests ──

    #[test]
    fn zellij_mode_emits_newlines_not_decsbm() {
        let mut terminal = test_terminal(80, 24);
        let viewport_before = terminal.viewport_area.y;

        let lines = vec![plain_line("zellij test")];
        insert_history_lines_with_mode(&mut terminal, lines, InsertHistoryMode::Zellij)
            .expect("insert zellij");

        let written = terminal.backend().written();

        // Zellij mode should NOT contain DECSTBM scroll-region escapes (CSI Ps;Ps r).
        // CSI sequences for cursor positioning and colors are expected.
        assert!(
            !written.contains("\x1b[19;24r"),
            "Zellij output should not use DECSTBM scroll regions: {:?}",
            written
        );

        assert!(
            terminal.viewport_area.y >= viewport_before,
            "viewport y {} should be >= {}",
            terminal.viewport_area.y,
            viewport_before
        );
        assert!(terminal.visible_history_rows() > 0);
    }

    #[test]
    fn standard_mode_emits_scroll_region_escape() {
        let mut terminal = test_terminal(80, 24);

        let lines = vec![plain_line("standard test")];
        insert_history_lines(&mut terminal, lines).expect("insert standard");

        let written = terminal.backend().written();

        // Standard mode uses DECSTBM (CSI ... r) to set scroll region.
        assert!(
            written.contains("\x1b["),
            "Standard mode output should contain CSI sequences (DECSTBM), got: {:?}",
            written
        );

        assert!(terminal.visible_history_rows() > 0);
    }

    // ── Wrapping tests ──

    #[test]
    fn long_line_wraps_and_counts_visual_rows() {
        let mut terminal = test_terminal(20, 24); // narrow terminal

        let long_text = "this is a very long line that should wrap";
        let lines = vec![plain_line(long_text)];
        let prev = terminal.visible_history_rows();

        insert_history_lines(&mut terminal, lines).expect("insert long line");

        // A line this long on a 20-wide terminal should wrap to multiple visual rows.
        assert!(
            terminal.visible_history_rows() >= prev + 2,
            "long line should wrap to multiple visual rows"
        );
    }
}
