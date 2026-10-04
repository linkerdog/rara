//! BottomPaneModel — owns all state consumed by the bottom pane renderer.
//!
//! Extracted from TuiApp to reduce coupling; the bottom pane should not
//! depend on the full TUI state machine.

use std::time::{Duration, Instant};

use super::{char_offset_to_byte_index, effective_cursor_offset};
use crate::tui::input_text::ceil_grapheme_offset;
use crate::tui::presentation_revision::PresentationInput;
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
    pub pending_planning_suggestion: PresentationInput<Option<String>>,
    pub pending_follow_up_messages: PresentationInput<Vec<PendingFollowUpMessage>>,
    pub queued_follow_up_messages: PresentationInput<Vec<String>>,
    pub running_task: Option<RunningTask>,
    pub notice: Option<String>,
    // Track the paste-owned notice so discarding a draft preserves newer warnings.
    pub(super) paste_notice: Option<String>,

    // Paste-burst state: when a paste contains newlines or exceeds the
    // large-paste threshold we accumulate chars and flush in one `push_str`,
    // avoiding O(n²) per-frame redraws for long pastes.
    pub(super) paste_burst_buffer: Option<String>,
    pub(super) paste_burst_deadline: Option<Instant>,
    /// Large pastes pending expansion on submit. Each entry is
    /// `(placeholder_text, full_text)` where placeholder is unique.
    pub(crate) large_paste_pending: Vec<(String, String)>,
    pub(crate) large_paste_counter: u32,
}

impl BottomPaneModel {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            input_cursor_offset: None,
            composer_scroll: 0,
            pending_planning_suggestion: Default::default(),
            pending_follow_up_messages: Default::default(),
            queued_follow_up_messages: Default::default(),
            running_task: None,
            notice: None,
            paste_notice: None,
            paste_burst_buffer: None,
            paste_burst_deadline: None,
            large_paste_pending: Vec::new(),
            large_paste_counter: 0,
        }
    }

    pub fn composer_cursor_offset(&self) -> usize {
        effective_cursor_offset(&self.input, self.input_cursor_offset)
    }

    pub(crate) fn clear_input(&mut self) {
        self.input.clear();
        self.input_cursor_offset = None;
        self.composer_scroll = 0;
        self.paste_burst_buffer = None;
        self.paste_burst_deadline = None;
        self.large_paste_pending.clear();
        self.large_paste_counter = 0;
        if let Some(paste_notice) = self.paste_notice.take()
            && self.notice.as_ref() == Some(&paste_notice)
        {
            self.notice = None;
        }
    }

    // ── Paste-burst ──────────────────────────────────────────────────

    /// Absorb a full paste chunk into the burst buffer rather than inserting
    /// char-by-char through the normal input path.
    pub fn handle_paste_burst_chunk(&mut self, chunk: &str) {
        self.paste_burst_buffer
            .get_or_insert_with(String::new)
            .push_str(chunk);
        self.paste_burst_deadline = Some(Instant::now() + PASTE_BURST_FLUSH_DELAY);
    }

    pub(crate) fn paste_burst_is_due(&self) -> bool {
        self.paste_burst_deadline
            .is_some_and(|deadline| Instant::now() >= deadline)
    }

    /// Force-flush any pending paste burst regardless of deadline.
    pub(crate) fn flush_paste_burst(&mut self) -> bool {
        let Some(buf) = self.paste_burst_buffer.take() else {
            return false;
        };
        self.paste_burst_deadline = None;

        let char_count = buf.chars().count();
        let is_large = char_count > 1000;

        if is_large {
            let counter = self.large_paste_counter;
            self.large_paste_counter += 1;
            let placeholder = format!("[Pasted Content #{} — {} chars]", counter, char_count);
            // Insert placeholder at cursor position instead of end
            let offset = self.composer_cursor_offset();
            let pos = char_offset_to_byte_index(&self.input, offset);
            self.input.insert_str(pos, &placeholder);
            self.input_cursor_offset = Some(ceil_grapheme_offset(
                &self.input,
                offset + placeholder.chars().count(),
            ));
            self.large_paste_pending.push((placeholder, buf));
            self.set_paste_notice(format!(
                "Large paste #{counter} ({char_count} chars) — expanded on submit"
            ));
            return true;
        }

        let paste_end = {
            let old_offset = self.composer_cursor_offset();
            if self.input_cursor_offset.is_none() {
                self.input.push_str(&buf);
                None
            } else {
                let pos = char_offset_to_byte_index(&self.input, old_offset);
                self.input.insert_str(pos, &buf);
                Some(ceil_grapheme_offset(
                    &self.input,
                    old_offset + buf.chars().count(),
                ))
            }
        };
        self.input_cursor_offset = paste_end;
        self.set_paste_notice(format!("Pasted {char_count} chars"));
        true
    }

    fn set_paste_notice(&mut self, notice: String) {
        self.notice = Some(notice.clone());
        self.paste_notice = Some(notice);
    }

    pub fn has_pending_planning_suggestion(&self) -> bool {
        self.pending_planning_suggestion.is_some()
    }

    /// Replace paste placeholder in input with the real text. Call before submit.
    pub(crate) fn expand_large_paste(&mut self) {
        let pending: Vec<_> = std::mem::take(&mut self.large_paste_pending);
        for (placeholder, full_text) in pending {
            self.input = self.input.replace(&placeholder, &full_text);
        }
        // Also handle legacy single-entry format for safety
        self.large_paste_counter = 0;
    }
}

impl Default for BottomPaneModel {
    fn default() -> Self {
        Self::new()
    }
}
