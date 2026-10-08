use std::io::{self, Write};

use ratatui::style::Color;

pub(super) enum ColorTarget {
    Foreground,
    Background,
}

/// Crossterm encodes named colors using 256-color SGR, even for ANSI-only TTYs.
pub(super) fn write_color(
    writer: &mut impl Write,
    target: ColorTarget,
    color: Color,
) -> io::Result<()> {
    #[cfg(windows)]
    if !crossterm::ansi_support::supports_ansi() {
        let color = crossterm_color(color);
        return match target {
            ColorTarget::Foreground => {
                crossterm::queue!(writer, crossterm::style::SetForegroundColor(color))
            }
            ColorTarget::Background => {
                crossterm::queue!(writer, crossterm::style::SetBackgroundColor(color))
            }
        };
    }
    let offset = match target {
        ColorTarget::Foreground => 0,
        ColorTarget::Background => 10,
    };
    let foreground_code = match color {
        Color::Reset => 39,
        Color::Black => 30,
        Color::Red => 31,
        Color::Green => 32,
        Color::Yellow => 33,
        Color::Blue => 34,
        Color::Magenta => 35,
        Color::Cyan => 36,
        Color::Gray => 37,
        Color::DarkGray => 90,
        Color::LightRed => 91,
        Color::LightGreen => 92,
        Color::LightYellow => 93,
        Color::LightBlue => 94,
        Color::LightMagenta => 95,
        Color::LightCyan => 96,
        Color::White => 97,
        Color::Rgb(r, g, b) => {
            return write!(writer, "\x1b[{};2;{r};{g};{b}m", 38 + offset);
        }
        Color::Indexed(index) => {
            return write!(writer, "\x1b[{};5;{index}m", 38 + offset);
        }
    };
    let code = foreground_code + offset;
    write!(writer, "\x1b[{code}m")
}

// Preserve crossterm's native fallback for Windows consoles without ANSI support.
#[cfg(windows)]
fn crossterm_color(color: Color) -> crossterm::style::Color {
    match color {
        Color::Reset => crossterm::style::Color::Reset,
        Color::Black => crossterm::style::Color::Black,
        Color::Red => crossterm::style::Color::DarkRed,
        Color::Green => crossterm::style::Color::DarkGreen,
        Color::Yellow => crossterm::style::Color::DarkYellow,
        Color::Blue => crossterm::style::Color::DarkBlue,
        Color::Magenta => crossterm::style::Color::DarkMagenta,
        Color::Cyan => crossterm::style::Color::DarkCyan,
        Color::Gray => crossterm::style::Color::Grey,
        Color::DarkGray => crossterm::style::Color::DarkGrey,
        Color::LightRed => crossterm::style::Color::Red,
        Color::LightGreen => crossterm::style::Color::Green,
        Color::LightYellow => crossterm::style::Color::Yellow,
        Color::LightBlue => crossterm::style::Color::Blue,
        Color::LightMagenta => crossterm::style::Color::Magenta,
        Color::LightCyan => crossterm::style::Color::Cyan,
        Color::White => crossterm::style::Color::White,
        Color::Rgb(r, g, b) => crossterm::style::Color::Rgb { r, g, b },
        Color::Indexed(value) => crossterm::style::Color::AnsiValue(value),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_use_basic_sgr_instead_of_palette_extensions() {
        let mut bytes = Vec::new();
        write_color(&mut bytes, ColorTarget::Foreground, Color::Red).unwrap();
        write_color(&mut bytes, ColorTarget::Background, Color::Blue).unwrap();
        write_color(&mut bytes, ColorTarget::Foreground, Color::LightGreen).unwrap();
        write_color(&mut bytes, ColorTarget::Background, Color::White).unwrap();
        write_color(&mut bytes, ColorTarget::Foreground, Color::Reset).unwrap();
        write_color(&mut bytes, ColorTarget::Background, Color::Reset).unwrap();
        assert_eq!(bytes, b"\x1b[31m\x1b[44m\x1b[92m\x1b[107m\x1b[39m\x1b[49m");
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "The terminal encoder test needs exact RGB and indexed fixtures."
    )]
    fn extended_colors_encode_the_already_resolved_profile() {
        let mut bytes = Vec::new();
        write_color(&mut bytes, ColorTarget::Foreground, Color::Rgb(1, 2, 3)).unwrap();
        write_color(&mut bytes, ColorTarget::Background, Color::Indexed(67)).unwrap();
        assert_eq!(bytes, b"\x1b[38;2;1;2;3m\x1b[48;5;67m");
    }
}
