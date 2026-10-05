use rara_terminal_detection::{GlyphSet, TerminalCapabilities};
use ratatui::buffer::Buffer;

use super::{terminal_glyphs, theme::TerminalPalette};

/// Adapt visible output only, leaving source text and retained rows unchanged.
pub(super) fn project_frame(buffer: &mut Buffer, capabilities: TerminalCapabilities) {
    if capabilities == TerminalCapabilities::FULL {
        return;
    }
    let mut palette = TerminalPalette::new(capabilities.colors);
    for cell in &mut buffer.content {
        palette.project_cell(cell);
        if capabilities.glyphs == GlyphSet::Ascii && !cell.symbol().is_ascii() {
            let symbol = terminal_glyphs::ascii_cell(cell.symbol());
            cell.set_symbol(&symbol);
        }
    }
}

#[cfg(test)]
#[path = "terminal_presentation_tests.rs"]
mod tests;
