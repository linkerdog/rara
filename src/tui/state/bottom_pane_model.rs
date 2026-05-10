//! BottomPaneModel — owns all state consumed by the bottom pane renderer.
//!
//! Extracted from TuiApp to reduce coupling; the bottom pane should not
//! depend on the full TUI state machine.

use unicode_width::UnicodeWidthChar;

use crate::tui::queued_input::PendingFollowUpMessage;
use crate::tui::state::types::RunningTask;

/// Shared helper that BottomPaneModel and TuiApp use for composer display.
pub fn composer_display_char_width(ch: char) -> usize {
    if ch.is_control() {
        return 1;
    }
    ch.width().unwrap_or(1).max(1)
}

#[derive(Debug)]
pub struct BottomPaneModel {
    pub input: String,
    pub input_cursor_offset: Option<usize>,
    pub composer_scroll: usize,
    pub pending_planning_suggestion: Option<String>,
    pub pending_follow_up_messages: Vec<PendingFollowUpMessage>,
    pub queued_follow_up_messages: Vec<String>,
    pub running_task: Option<RunningTask>,
    pub notice: Option<String>,
}

impl BottomPaneModel {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            input_cursor_offset: None,
            composer_scroll: 0,
            pending_planning_suggestion: None,
            pending_follow_up_messages: Vec::new(),
            queued_follow_up_messages: Vec::new(),
            running_task: None,
            notice: None,
        }
    }

    pub fn composer_cursor_offset(&self) -> usize {
        let text = &self.input;
        self.input_cursor_offset
            .unwrap_or_else(|| text.chars().count())
            .min(text.chars().count())
    }

    pub fn maintain_composer_scroll(&mut self, _composer_width: u16, visible_height: u16) {
        let cursor_offset = self.composer_cursor_offset();
        let mut cursor_line = 0usize;
        for (i, ch) in self.input.chars().enumerate() {
            if i == cursor_offset {
                break;
            }
            if ch == '\n' {
                cursor_line += 1;
            }
        }
        let visible_height = visible_height.max(1) as usize;
        let end_line = self.composer_scroll + visible_height - 1;
        if cursor_line < self.composer_scroll {
            self.composer_scroll = cursor_line;
        } else if cursor_line > end_line {
            self.composer_scroll = cursor_line - visible_height + 1;
        }
    }

    pub fn has_pending_planning_suggestion(&self) -> bool {
        self.pending_planning_suggestion.is_some()
    }

    pub fn has_queued_follow_up_messages(&self) -> bool {
        !self.queued_follow_up_messages.is_empty()
    }

    pub fn has_pending_follow_up_messages(&self) -> bool {
        !self.pending_follow_up_messages.is_empty()
    }
}

impl Default for BottomPaneModel {
    fn default() -> Self {
        Self::new()
    }
}
