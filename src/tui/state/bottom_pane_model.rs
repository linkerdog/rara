//! BottomPaneModel — owns all state consumed by the bottom pane renderer.
//!
//! Extracted from TuiApp to reduce coupling; the bottom pane should not
//! depend on the full TUI state machine.

use std::time::{Duration, Instant};

use super::char_offset_to_byte_index;
use crate::tui::queued_input::PendingFollowUpMessage;
use crate::tui::state::types::RunningTask;

/// Time window after the last paste-char before the accumulated burst is
/// flushed into the composer.
const PASTE_BURST_FLUSH_DELAY: Duration = Duration::from_millis(500);

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

    // Paste-burst state: when a paste contains newlines or exceeds the
    // large-paste threshold we accumulate chars and flush in one `push_str`,
    // avoiding O(n²) per-frame redraws for long pastes.
    pub(super) paste_burst_buffer: Option<String>,
    pub(super) paste_burst_deadline: Option<Instant>,
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
            paste_burst_buffer: None,
            paste_burst_deadline: None,
        }
    }

    pub fn composer_cursor_offset(&self) -> usize {
        let text = &self.input;
        self.input_cursor_offset
            .unwrap_or_else(|| text.chars().count())
            .min(text.chars().count())
    }

    // ── Paste-burst ──────────────────────────────────────────────────

    /// Whether the composer is currently in a paste burst — i.e. chars are
    /// being accumulated rather than applied one at a time.
    pub fn is_in_paste_burst(&self) -> bool {
        self.paste_burst_buffer.is_some()
    }

    /// Absorb a full paste chunk into the burst buffer rather than inserting
    /// char-by-char through the normal input path.
    pub fn handle_paste_burst_chunk(&mut self, chunk: &str) {
        self.paste_burst_buffer
            .get_or_insert_with(String::new)
            .push_str(chunk);
        self.paste_burst_deadline = Some(Instant::now() + PASTE_BURST_FLUSH_DELAY);
    }

    /// Flush completed paste bursts into the input string.
    ///
    /// Returns `true` when a burst was flushed (caller should redraw).
    pub fn check_paste_burst_flush(&mut self) -> bool {
        let Some(deadline) = self.paste_burst_deadline else {
            return false;
        };
        if Instant::now() < deadline {
            return false;
        }
        self.flush_paste_burst()
    }

    /// Force-flush any pending paste burst regardless of deadline.
    pub(crate) fn flush_paste_burst(&mut self) -> bool {
        let Some(buf) = self.paste_burst_buffer.take() else {
            return false;
        };
        self.paste_burst_deadline = None;
        let paste_end = {
            let old_offset = self.composer_cursor_offset();
            if self.input_cursor_offset.is_none() {
                self.input.push_str(&buf);
                None
            } else {
                let pos = char_offset_to_byte_index(&self.input, old_offset);
                self.input.insert_str(pos, &buf);
                Some(old_offset + buf.chars().count())
            }
        };
        self.input_cursor_offset = paste_end;
        true
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
