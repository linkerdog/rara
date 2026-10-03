use std::io::{self, Write};

use crossterm::style::{Print, ResetColor, SetAttribute};
use crossterm::terminal::{BeginSynchronizedUpdate, EndSynchronizedUpdate};
use crossterm::{execute, queue};
use ratatui::backend::Backend;
use ratatui::layout::{Position, Rect};

use super::{Frame, Terminal};

impl<B: Backend<Error = io::Error> + Write> Terminal<B> {
    /// Reserve, resize, paint, and place the cursor in one synchronized update.
    pub(crate) fn draw_inline<F>(&mut self, render: F) -> io::Result<()>
    where
        F: FnOnce(&mut Frame),
    {
        queue!(self.backend, BeginSynchronizedUpdate)?;
        let result = (|| {
            let size = self.size()?;
            let area = Rect::new(0, 0, size.width, size.height);
            let reserved = !self.inline_viewport_owned && !area.is_empty();
            if reserved {
                // Preserve the shell's current line as well as prior output.
                // Newlines scroll it into history instead of erasing it with ED2.
                // Relative output also works when the startup cursor query fails.
                queue!(self.backend, Print("\r"))?;
                self.inline_viewport_owned = true;
                self.backend.append_lines(size.height)?;
            }
            if reserved || area != self.viewport_area {
                self.set_viewport_area(area);
                self.invalidate_viewport();
            }
            self.draw(render)
        })();
        let end = execute!(self.backend, EndSynchronizedUpdate);
        match (result, end) {
            (Err(error), Err(cleanup)) => {
                log::warn!("Failed to finish synchronized terminal update: {cleanup}");
                Err(error)
            }
            (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
            (Ok(()), Ok(())) => Ok(()),
        }
    }

    /// Repaint after terminal reflow, even if a resize burst ends at the old size.
    pub(crate) fn invalidate_viewport(&mut self) {
        // Empty first-column cells must also overwrite stale terminal contents.
        // This marker is only in the diff input and never reaches the backend.
        for cell in &mut self.previous_buffer_mut().content {
            cell.set_symbol("\0");
        }
    }

    /// Hand the shell a clean line below the last frame, once per reservation.
    pub(crate) fn finish_inline_viewport(&mut self) -> io::Result<()> {
        if !std::mem::take(&mut self.inline_viewport_owned) {
            return Ok(());
        }
        let size = self.size()?;
        if size.height == 0 {
            return Ok(());
        }
        // A resize may arrive between the last frame and shutdown. Reflow can
        // move that frame to the new bottom edge before we receive its event.
        let row = size.height - 1;
        self.set_cursor_position((0, row))?;
        queue!(
            self.backend,
            SetAttribute(crossterm::style::Attribute::Reset),
            ResetColor,
            Print("\r\n")
        )?;
        self.last_known_cursor_pos = Position::new(0, row.saturating_add(1).min(size.height - 1));
        self.show_cursor()?;
        Backend::flush(&mut self.backend)
    }
}

#[cfg(test)]
#[path = "inline_tests.rs"]
mod tests;
