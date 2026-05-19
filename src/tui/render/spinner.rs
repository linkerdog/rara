
//! Fixed-width activity spinner.
//!
//! Returns a 1-character Span whose glyph and color animate over time
//! but whose width never changes, so it does not shift adjacent layout.

use std::time::Duration;

use ratatui::{
    style::{Color, Style},
    text::Span,
};

// Nord palette reference: https://www.nordtheme.com/docs/colors-and-palettes
const SHIMMER_COLORS: &[Color] = &[
    Color::Rgb(0x88, 0xC0, 0xD0), // NORD8  – frost cyan
    Color::Rgb(0x8F, 0xBC, 0xBB), // NORD7  – frost green-cyan
    Color::Rgb(0xA3, 0xBE, 0x8C), // NORD14 – aurora green
    Color::Rgb(0xEB, 0xCB, 0x8B), // NORD13 – aurora yellow
    Color::Rgb(0xD0, 0x87, 0x70), // NORD12 – aurora orange
    Color::Rgb(0x81, 0xA1, 0xC1), // NORD9  – frost blue-gray
];

/// Fixed-width 1-character spinner.
///
/// When `active` is false returns a plain space so the activity bar shows no
/// spinner glyph.
///
/// When `active` the dot `•` is drawn with a colour that slowly cycles
/// through the Nord frost/aurora palette.  The span is always exactly one
/// character wide, so surrounding layout never reflows.
pub(crate) fn spinner(active: bool, elapsed: Duration) -> Span<'static> {
    if !active {
        return Span::raw(" ");
    }
    let cycle_step = (elapsed.as_millis() / 160) as usize;
    let color = SHIMMER_COLORS[cycle_step % SHIMMER_COLORS.len()];
    Span::styled("•", Style::default().fg(color))
}
