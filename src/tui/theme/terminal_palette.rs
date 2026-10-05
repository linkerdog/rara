use std::collections::HashMap;

use rara_terminal_detection::ColorLevel;
use ratatui::{buffer::Cell, style::Color};

const ANSI: [(Color, (u8, u8, u8)); 16] = [
    (Color::Black, (0, 0, 0)),
    (Color::Red, (128, 0, 0)),
    (Color::Green, (0, 128, 0)),
    (Color::Yellow, (128, 128, 0)),
    (Color::Blue, (0, 0, 128)),
    (Color::Magenta, (128, 0, 128)),
    (Color::Cyan, (0, 128, 128)),
    (Color::Gray, (192, 192, 192)),
    (Color::DarkGray, (128, 128, 128)),
    (Color::LightRed, (255, 0, 0)),
    (Color::LightGreen, (0, 255, 0)),
    (Color::LightYellow, (255, 255, 0)),
    (Color::LightBlue, (0, 0, 255)),
    (Color::LightMagenta, (255, 0, 255)),
    (Color::LightCyan, (0, 255, 255)),
    (Color::White, (255, 255, 255)),
];

/// Resolve each distinct visible color once per frame, including syntax colors.
pub(crate) struct TerminalPalette {
    level: ColorLevel,
    resolved: HashMap<Color, Color>,
}

impl TerminalPalette {
    pub(crate) fn new(level: ColorLevel) -> Self {
        Self {
            level,
            resolved: HashMap::new(),
        }
    }

    pub(crate) fn project_cell(&mut self, cell: &mut Cell) {
        let distinct = cell.fg != cell.bg;
        cell.fg = self.resolve(cell.fg);
        cell.bg = self.resolve(cell.bg);
        cell.underline_color = self.resolve(cell.underline_color);
        // Muted foregrounds and dark surfaces can quantize to the same ANSI slot.
        if matches!(
            self.level,
            ColorLevel::Ansi8 | ColorLevel::Ansi16 | ColorLevel::Ansi256
        ) && distinct
            && cell.fg == cell.bg
            && cell.fg != Color::Reset
        {
            let (r, g, b) = rgb(cell.bg);
            cell.fg = if u32::from(r) * 299 + u32::from(g) * 587 + u32::from(b) * 114 >= 128_000 {
                Color::Black
            } else {
                Color::Gray
            };
        }
    }

    fn resolve(&mut self, color: Color) -> Color {
        match self.level {
            ColorLevel::TrueColor => color,
            ColorLevel::Monochrome => Color::Reset,
            ColorLevel::Ansi8 | ColorLevel::Ansi16 | ColorLevel::Ansi256 => {
                if color == Color::Reset {
                    return color;
                }
                *self
                    .resolved
                    .entry(color)
                    .or_insert_with(|| match self.level {
                        ColorLevel::Ansi8 => nearest_ansi(rgb(color), 8),
                        ColorLevel::Ansi16 => nearest_ansi(rgb(color), 16),
                        ColorLevel::Ansi256 => match color {
                            Color::Rgb(r, g, b) => nearest_fixed((r, g, b)),
                            _ => color,
                        },
                        ColorLevel::TrueColor | ColorLevel::Monochrome => color,
                    })
            }
        }
    }
}

fn nearest_ansi(target: (u8, u8, u8), count: usize) -> Color {
    ANSI[..count]
        .iter()
        .min_by_key(|(_, rgb)| distance(target, *rgb))
        .map_or(Color::Reset, |(color, _)| *color)
}

#[expect(
    clippy::disallowed_methods,
    reason = "The terminal palette owner quantizes RGB into supported fixed ANSI slots."
)]
fn nearest_fixed(target: (u8, u8, u8)) -> Color {
    (16..=255)
        .min_by_key(|index| distance(target, indexed_rgb(*index)))
        .map_or(Color::Reset, Color::Indexed)
}

fn distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let r = i32::from(a.0) - i32::from(b.0);
    let g = i32::from(a.1) - i32::from(b.1);
    let b = i32::from(a.2) - i32::from(b.2);
    (r * r + g * g + b * b) as u32
}

fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    match index {
        0..=15 => ANSI[usize::from(index)].1,
        16..=231 => {
            let steps = [0, 95, 135, 175, 215, 255];
            let offset = usize::from(index - 16);
            (
                steps[offset / 36],
                steps[(offset / 6) % 6],
                steps[offset % 6],
            )
        }
        232..=255 => {
            let gray = 8 + (index - 232) * 10;
            (gray, gray, gray)
        }
    }
}

fn rgb(color: Color) -> (u8, u8, u8) {
    match color {
        Color::Reset => (0, 0, 0),
        Color::Black => ANSI[0].1,
        Color::Red => ANSI[1].1,
        Color::Green => ANSI[2].1,
        Color::Yellow => ANSI[3].1,
        Color::Blue => ANSI[4].1,
        Color::Magenta => ANSI[5].1,
        Color::Cyan => ANSI[6].1,
        Color::Gray => ANSI[7].1,
        Color::DarkGray => ANSI[8].1,
        Color::LightRed => ANSI[9].1,
        Color::LightGreen => ANSI[10].1,
        Color::LightYellow => ANSI[11].1,
        Color::LightBlue => ANSI[12].1,
        Color::LightMagenta => ANSI[13].1,
        Color::LightCyan => ANSI[14].1,
        Color::White => ANSI[15].1,
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_rgb(index),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Modifier;

    use super::*;

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "The palette conversion test asserts exact RGB and indexed input/output pairs."
    )]
    fn each_palette_obeys_its_color_ceiling() {
        for (level, expected) in [
            (ColorLevel::Monochrome, Color::Reset),
            (ColorLevel::Ansi8, Color::Red),
            (ColorLevel::Ansi16, Color::LightRed),
            (ColorLevel::Ansi256, Color::Indexed(196)),
            (ColorLevel::TrueColor, Color::Rgb(255, 0, 0)),
        ] {
            assert_eq!(
                TerminalPalette::new(level).resolve(Color::Rgb(255, 0, 0)),
                expected
            );
        }
        for index in 0..=255 {
            let color = TerminalPalette::new(ColorLevel::Ansi8).resolve(Color::Indexed(index));
            assert!(ANSI[..8].iter().any(|(named, _)| color == *named));
            let color = TerminalPalette::new(ColorLevel::Ansi16).resolve(Color::Indexed(index));
            assert!(ANSI.iter().any(|(named, _)| color == *named));
        }
        assert_eq!(
            TerminalPalette::new(ColorLevel::Ansi256).resolve(Color::Rgb(95, 135, 175)),
            Color::Indexed(67)
        );
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "The monochrome test needs non-default colors in every color channel."
    )]
    fn monochrome_preserves_all_non_color_cell_attributes() {
        let mut cell = Cell::new("x");
        cell.fg = Color::Rgb(80, 90, 100);
        cell.bg = Color::Indexed(30);
        cell.underline_color = Color::Red;
        cell.modifier = Modifier::BOLD | Modifier::DIM | Modifier::REVERSED;
        TerminalPalette::new(ColorLevel::Monochrome).project_cell(&mut cell);
        assert_eq!(
            (cell.fg, cell.bg, cell.underline_color),
            (Color::Reset, Color::Reset, Color::Reset)
        );
        assert_eq!(
            cell.modifier,
            Modifier::BOLD | Modifier::DIM | Modifier::REVERSED
        );
        assert_eq!(cell.symbol(), "x");
    }

    #[test]
    #[expect(
        clippy::disallowed_methods,
        reason = "The contrast test requires distinct RGB values that share an ANSI slot."
    )]
    fn quantized_muted_text_remains_visible_on_a_dark_surface() {
        let mut cell = Cell::new("x");
        cell.fg = Color::Rgb(20, 20, 20);
        cell.bg = Color::Rgb(10, 10, 10);
        let mut palette = TerminalPalette::new(ColorLevel::Ansi8);
        palette.project_cell(&mut cell);
        assert_eq!((cell.fg, cell.bg), (Color::Gray, Color::Black));
        let first = cell.clone();
        palette.project_cell(&mut cell);
        assert_eq!(cell, first, "projection must be idempotent");
    }
}
