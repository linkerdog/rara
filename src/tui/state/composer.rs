use super::types::{Overlay, TuiApp};
use super::{
    INPUT_HISTORY_LIMIT, TextInputTarget, char_offset_to_byte_index, effective_cursor_offset,
};
use crate::tui::input_text::{
    ceil_grapheme_offset, floor_grapheme_offset, next_grapheme_offset, previous_grapheme_offset,
};

impl TuiApp {
    fn active_text_input_target(&self) -> Option<TextInputTarget> {
        match self.overlay {
            Some(Overlay::Goal) => matches!(
                self.goal_ui.dialog,
                Some(crate::tui::goal_ui::GoalDialog::Edit(_))
            )
            .then_some(TextInputTarget::GoalObjective),
            None | Some(Overlay::CommandPalette) => Some(TextInputTarget::Composer),
            Some(Overlay::ModelSearch) => Some(TextInputTarget::ModelSearch),
            Some(Overlay::ListPicker(super::ListPickerKind::Resume)) => {
                Some(TextInputTarget::ResumeSearch)
            }
            Some(Overlay::BaseUrlEditor) => Some(TextInputTarget::BaseUrl),
            Some(Overlay::ApiKeyEditor(_)) => Some(TextInputTarget::ApiKey),
            Some(Overlay::ModelNameEditor) => Some(TextInputTarget::ModelName),
            Some(Overlay::OpenAiProfileLabelEditor) => Some(TextInputTarget::OpenAiProfileLabel),
            Some(
                Overlay::Help(_)
                | Overlay::Status(_)
                | Overlay::Context
                | Overlay::SkillsPicker
                | Overlay::PermissionPicker
                | Overlay::ListPicker(_),
            ) => None,
        }
    }

    pub(crate) fn composer_input_is_active(&self) -> bool {
        self.active_text_input_target() == Some(TextInputTarget::Composer)
    }

    pub(crate) fn flush_composer_paste(&mut self) -> bool {
        let flushed = self.bottom_pane.flush_paste_burst();
        if flushed {
            self.update_after_active_input_edit(TextInputTarget::Composer);
        }
        flushed
    }

    pub(crate) fn check_composer_paste_flush(&mut self) -> bool {
        self.bottom_pane.paste_burst_is_due() && self.flush_composer_paste()
    }

    fn text_and_cursor_mut(
        &mut self,
        target: TextInputTarget,
    ) -> (&mut String, &mut Option<usize>) {
        match target {
            TextInputTarget::ResumeSearch => (
                &mut self.resume_search_query,
                &mut self.resume_search_cursor_offset,
            ),
            TextInputTarget::GoalObjective => (&mut self.goal_ui.input, &mut self.goal_ui.cursor),
            TextInputTarget::Composer => (
                &mut self.bottom_pane.input,
                &mut self.bottom_pane.input_cursor_offset,
            ),
            TextInputTarget::ModelSearch => (
                &mut self.model_search_query,
                &mut self.model_search_cursor_offset,
            ),
            TextInputTarget::BaseUrl => {
                (&mut self.base_url_input, &mut self.base_url_cursor_offset)
            }
            TextInputTarget::ApiKey => (&mut self.api_key_input, &mut self.api_key_cursor_offset),
            TextInputTarget::ModelName => (
                &mut self.model_name_input,
                &mut self.model_name_cursor_offset,
            ),
            TextInputTarget::OpenAiProfileLabel => (
                &mut self.openai_profile_label_input,
                &mut self.openai_profile_label_cursor_offset,
            ),
        }
    }

    fn update_after_active_input_edit(&mut self, target: TextInputTarget) {
        match target {
            TextInputTarget::Composer => {
                self.reset_input_history_navigation();
                self.sync_command_palette_with_input();
            }
            TextInputTarget::ModelSearch => self.model_search_idx = 0,
            TextInputTarget::ResumeSearch => self.resume_search_changed(),
            TextInputTarget::GoalObjective
            | TextInputTarget::BaseUrl
            | TextInputTarget::ApiKey
            | TextInputTarget::ModelName
            | TextInputTarget::OpenAiProfileLabel => {}
        }
    }

    pub(crate) fn model_search_cursor_offset(&self) -> usize {
        effective_cursor_offset(&self.model_search_query, self.model_search_cursor_offset)
    }

    pub(crate) fn resume_search_cursor_offset(&self) -> usize {
        effective_cursor_offset(&self.resume_search_query, self.resume_search_cursor_offset)
    }

    /// Returns the slash-command query string with the leading `/` stripped.
    /// For example, if the input is `/help`, this returns `"help"`.
    pub fn command_query(&self) -> &str {
        self.bottom_pane.input.trim_start().trim_start_matches('/')
    }

    pub fn composer_cursor_offset(&self) -> usize {
        effective_cursor_offset(
            self.bottom_pane.input.as_str(),
            self.bottom_pane.input_cursor_offset,
        )
    }

    pub fn base_url_cursor_offset(&self) -> usize {
        effective_cursor_offset(self.base_url_input.as_str(), self.base_url_cursor_offset)
    }

    pub fn api_key_cursor_offset(&self) -> usize {
        effective_cursor_offset(self.api_key_input.as_str(), self.api_key_cursor_offset)
    }

    pub fn model_name_cursor_offset(&self) -> usize {
        effective_cursor_offset(
            self.model_name_input.as_str(),
            self.model_name_cursor_offset,
        )
    }

    pub fn openai_profile_label_cursor_offset(&self) -> usize {
        effective_cursor_offset(
            self.openai_profile_label_input.as_str(),
            self.openai_profile_label_cursor_offset,
        )
    }

    /// Test helper for seeding composer input without simulating key events.
    #[allow(dead_code)] // Reserved for programmatic input control
    pub fn set_input(&mut self, input: String) {
        self.bottom_pane.input = input;
        self.bottom_pane.input_cursor_offset = None;
        self.reset_input_history_navigation();
        self.sync_command_palette_with_input();
    }

    fn set_input_from_history(&mut self, input: String) {
        self.bottom_pane.input = input;
        self.bottom_pane.input_cursor_offset = Some(self.bottom_pane.input.chars().count());
        self.sync_command_palette_with_input();
    }

    pub fn record_input_history(&mut self, input: &str) {
        let input = input.trim();
        if input.is_empty() {
            return;
        }
        if self
            .input_history
            .last()
            .is_some_and(|previous| previous == input)
        {
            self.reset_input_history_navigation();
            return;
        }
        self.input_history.push(input.to_string());
        if self.input_history.len() > INPUT_HISTORY_LIMIT {
            let excess = self.input_history.len() - INPUT_HISTORY_LIMIT;
            self.input_history.drain(..excess);
        }
        self.reset_input_history_navigation();
    }

    pub fn reset_input_history_navigation(&mut self) {
        self.input_history_cursor = None;
        self.input_history_draft = None;
    }

    pub fn should_handle_input_history_navigation(&self, delta: i32) -> bool {
        if self.input_history.is_empty() {
            return false;
        }
        if self.bottom_pane.input.is_empty() {
            return true;
        }
        let cursor = self.composer_cursor_offset();
        if delta < 0 {
            cursor == 0 || self.input_history_cursor.is_some()
        } else {
            delta > 0
                && cursor == self.bottom_pane.input.chars().count()
                && self.input_history_cursor.is_some()
        }
    }

    pub fn navigate_input_history(&mut self, delta: i32) {
        if self.input_history.is_empty() || delta == 0 {
            return;
        }

        let next = match self.input_history_cursor {
            None if delta < 0 => {
                self.input_history_draft = Some(self.bottom_pane.input.clone());
                Some(self.input_history.len().saturating_sub(1))
            }
            None => return,
            Some(idx) if delta < 0 => Some(idx.saturating_sub(1)),
            Some(idx) if idx + 1 < self.input_history.len() => Some(idx + 1),
            Some(_) => None,
        };

        match next {
            Some(idx) => {
                self.input_history_cursor = Some(idx);
                if let Some(entry) = self.input_history.get(idx).cloned() {
                    self.set_input_from_history(entry);
                }
            }
            None => {
                let draft = self.input_history_draft.take().unwrap_or_default();
                self.input_history_cursor = None;
                self.set_input_from_history(draft);
            }
        }
    }

    pub fn insert_active_input_char(&mut self, ch: char) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        let byte_idx = char_offset_to_byte_index(text.as_str(), cursor);
        text.insert(byte_idx, ch);
        *cursor_offset = Some(ceil_grapheme_offset(text, cursor.saturating_add(1)));
        self.update_after_active_input_edit(target);
    }

    pub fn insert_active_input_text(&mut self, inserted: &str) {
        if inserted.is_empty() {
            return;
        }
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        let byte_idx = char_offset_to_byte_index(text.as_str(), cursor);
        text.insert_str(byte_idx, inserted);
        *cursor_offset = Some(ceil_grapheme_offset(
            text,
            cursor.saturating_add(inserted.chars().count()),
        ));
        self.update_after_active_input_edit(target);
    }

    pub fn insert_newline_in_composer(&mut self) {
        let cursor = self.composer_cursor_offset();
        let byte_idx = char_offset_to_byte_index(self.bottom_pane.input.as_str(), cursor);
        self.bottom_pane.input.insert(byte_idx, '\n');
        self.bottom_pane.input_cursor_offset = Some(ceil_grapheme_offset(
            &self.bottom_pane.input,
            cursor.saturating_add(1),
        ));
        self.sync_command_palette_with_input();
    }

    /// Keep the composer cursor visible by adjusting `composer_scroll`.
    ///
    /// `wrapped_cursor_row` and `wrapped_total_rows` come from the
    /// renderer's soft-wrap computation so the scroll tracks the visual
    /// display rather than only hard newlines.
    pub fn maintain_composer_scroll(
        &mut self,
        _composer_width: u16,
        visible_height: u16,
        wrapped_cursor_row: usize,
        wrapped_total_rows: usize,
    ) {
        let height = visible_height.max(1) as usize;
        let max_scroll = wrapped_total_rows.saturating_sub(1);
        if wrapped_cursor_row < self.bottom_pane.composer_scroll {
            self.bottom_pane.composer_scroll = wrapped_cursor_row;
        } else if wrapped_cursor_row >= self.bottom_pane.composer_scroll + height {
            self.bottom_pane.composer_scroll =
                (wrapped_cursor_row.saturating_sub(height - 1)).min(max_scroll);
        }
        self.bottom_pane.composer_scroll = self.bottom_pane.composer_scroll.min(max_scroll);
    }

    fn composer_text_layout(&self) -> std::sync::Arc<crate::tui::composer_text::WrappedText> {
        let columns = crate::tui::pane_geometry::PaneColumns {
            terminal_width: self.terminal_width,
            sidebar_visible: self.sidebar_visible,
        };
        crate::tui::composer_text::wrapped_text(
            &self.bottom_pane.input,
            crate::tui::composer_text::WrapConfig::composer(columns.main_width()),
        )
    }

    pub fn backspace_active_input(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        if cursor == 0 {
            return;
        }
        let previous = previous_grapheme_offset(text, cursor);
        let start = char_offset_to_byte_index(text.as_str(), previous);
        let end = char_offset_to_byte_index(text.as_str(), cursor);
        text.replace_range(start..end, "");
        *cursor_offset = Some(floor_grapheme_offset(text, previous));
        self.update_after_active_input_edit(target);
    }

    pub fn delete_forward_active_input(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        if cursor >= text.chars().count() {
            return;
        }
        let start = char_offset_to_byte_index(text.as_str(), cursor);
        let end = char_offset_to_byte_index(text.as_str(), next_grapheme_offset(text, cursor));
        text.replace_range(start..end, "");
        *cursor_offset = Some(floor_grapheme_offset(text, cursor));
        self.update_after_active_input_edit(target);
    }

    pub fn move_active_input_cursor_left(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        *cursor_offset = Some(previous_grapheme_offset(text, cursor));
    }

    pub fn move_active_input_cursor_right(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        *cursor_offset = Some(next_grapheme_offset(text, cursor));
    }

    pub fn move_active_input_cursor_home(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (_, cursor_offset) = self.text_and_cursor_mut(target);
        *cursor_offset = Some(0);
    }

    pub fn move_active_input_cursor_end(&mut self) {
        let Some(target) = self.active_text_input_target() else {
            return;
        };
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        *cursor_offset = Some(text.chars().count());
    }

    pub fn move_composer_cursor_up(&mut self) {
        let cursor = self.composer_cursor_offset();
        let layout = self.composer_text_layout();
        let position = layout.position_for_offset(cursor);
        if position.row == 0 {
            self.bottom_pane.input_cursor_offset = Some(0);
            return;
        }
        self.bottom_pane.input_cursor_offset = Some(layout.offset_for_position(
            crate::tui::composer_text::VisualPosition {
                row: position.row - 1,
                column: position.column,
            },
        ));
    }

    pub fn move_composer_cursor_down(&mut self) {
        let cursor = self.composer_cursor_offset();
        let layout = self.composer_text_layout();
        let position = layout.position_for_offset(cursor);
        let target = layout.offset_for_position(crate::tui::composer_text::VisualPosition {
            row: position.row + 1,
            column: position.column,
        });
        self.bottom_pane.input_cursor_offset = Some(target);
    }
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use crate::tui::state::RuntimeSnapshot;
    use crate::tui::testing::TuiHarness;

    #[test]
    fn timed_paste_flush_resets_history_navigation_without_sleeping() {
        let mut tui = TuiHarness::new(RuntimeSnapshot::default()).expect("isolated harness");
        let app = tui.app_mut();
        app.record_input_history("old prompt");
        app.navigate_input_history(-1);
        assert!(app.input_history_cursor.is_some());
        app.bottom_pane.handle_paste_burst_chunk("first\nsecond");
        app.bottom_pane.paste_burst_deadline = Some(Instant::now());
        assert!(app.check_composer_paste_flush());
        assert_eq!(app.bottom_pane.input, "old promptfirst\nsecond");
        assert!(app.input_history_cursor.is_none());
        assert!(!app.check_composer_paste_flush());
        app.bottom_pane.clear_input();
        assert!(app.bottom_pane.notice.is_none());
        assert!(app.bottom_pane.paste_burst_deadline.is_none());
        assert!(!app.check_composer_paste_flush());
    }
}
