//! Fixed-width activity spinner.
//!
//! Returns a 1-character Span whose glyph and color animate over time
//! but whose width never changes, so it does not shift adjacent layout.

use std::time::Duration;

use ratatui::{style::Style, text::Span};

use crate::tui::theme::{NORD7, NORD8, NORD9, NORD12, NORD13, NORD14};

// Nord palette reference: https://www.nordtheme.com/docs/colors-and-palettes
const SHIMMER_COLORS: &[ratatui::style::Color] = &[NORD8, NORD7, NORD14, NORD13, NORD12, NORD9];

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
