use super::types::{Overlay, TuiApp};
use super::{
    INPUT_HISTORY_LIMIT, TextInputTarget, char_offset_to_byte_index, composer_display_char_width,
    effective_cursor_offset,
};

impl TuiApp {
    fn active_text_input_target(&self) -> TextInputTarget {
        match self.overlay {
            Some(Overlay::BaseUrlEditor) => TextInputTarget::BaseUrl,
            Some(Overlay::ApiKeyEditor) => TextInputTarget::ApiKey,
            Some(Overlay::ModelNameEditor) => TextInputTarget::ModelName,
            Some(Overlay::OpenAiProfileLabelEditor) => TextInputTarget::OpenAiProfileLabel,
            _ => TextInputTarget::Composer,
        }
    }

    fn text_and_cursor_mut(
        &mut self,
        target: TextInputTarget,
    ) -> (&mut String, &mut Option<usize>) {
        match target {
            TextInputTarget::Composer => (
                &mut self.bottom_pane.input,
                &mut self.bottom_pane.input_cursor_offset,
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

    fn update_composer_after_edit_if_needed(&mut self, target: TextInputTarget) {
        if matches!(target, TextInputTarget::Composer) {
            self.reset_input_history_navigation();
            self.sync_command_palette_with_input();
        }
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
    #[allow(dead_code)]
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
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        let byte_idx = char_offset_to_byte_index(text.as_str(), cursor);
        text.insert(byte_idx, ch);
        *cursor_offset = Some(cursor.saturating_add(1));
        self.update_composer_after_edit_if_needed(target);
    }

    pub fn insert_active_input_text(&mut self, inserted: &str) {
        if inserted.is_empty() {
            return;
        }
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        let byte_idx = char_offset_to_byte_index(text.as_str(), cursor);
        text.insert_str(byte_idx, inserted);
        *cursor_offset = Some(cursor.saturating_add(inserted.chars().count()));
        self.update_composer_after_edit_if_needed(target);
    }

    pub fn insert_newline_in_composer(&mut self) {
        let cursor = self.composer_cursor_offset();
        let byte_idx = char_offset_to_byte_index(self.bottom_pane.input.as_str(), cursor);
        self.bottom_pane.input.insert(byte_idx, '\n');
        self.bottom_pane.input_cursor_offset = Some(cursor.saturating_add(1));
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

    fn composer_visual_position_for_offset(&self, cursor_offset: usize) -> (usize, usize) {
        let max_width = self.terminal_width.max(1) as usize;
        let mut row = 0usize;
        let mut column = 2usize;
        let mut content_width = 0usize;

        for (seen, ch) in self.bottom_pane.input.chars().enumerate() {
            if seen >= cursor_offset {
                break;
            }

            if ch == '\n' {
                row += 1;
                column = 2;
                content_width = 0;
                continue;
            }

            let char_width = composer_display_char_width(ch);
            if 2usize
                .saturating_add(content_width)
                .saturating_add(char_width)
                > max_width
                && content_width > 0
            {
                row += 1;
                content_width = 0;
            }

            content_width = content_width.saturating_add(char_width);
            column = 2usize.saturating_add(content_width);
        }

        (row, column)
    }

    fn composer_offset_for_visual_position(
        &self,
        target_row: usize,
        target_column: usize,
    ) -> usize {
        let max_width = self.terminal_width.max(1) as usize;
        let mut row = 0usize;
        let mut column = 2usize;
        let mut content_width = 0usize;
        let mut current_offset = 0usize;
        let mut best_offset = 0usize;
        let mut best_distance = usize::MAX;

        for ch in self.bottom_pane.input.chars() {
            if row != target_row {
                if row > target_row {
                    return best_offset;
                }
            } else {
                let distance = column.abs_diff(target_column);
                if distance < best_distance
                    || (distance == best_distance && current_offset > best_offset)
                {
                    best_distance = distance;
                    best_offset = current_offset;
                }
            }

            current_offset += 1;

            if ch == '\n' {
                row += 1;
                column = 2;
                content_width = 0;
                continue;
            }

            let char_width = composer_display_char_width(ch);
            if 2usize
                .saturating_add(content_width)
                .saturating_add(char_width)
                > max_width
                && content_width > 0
            {
                row += 1;
                content_width = 0;
            }

            content_width = content_width.saturating_add(char_width);
            column = 2usize.saturating_add(content_width);
        }

        if row == target_row {
            let distance = column.abs_diff(target_column);
            if distance < best_distance
                || (distance == best_distance && current_offset > best_offset)
            {
                best_offset = current_offset;
            }
            return best_offset;
        }

        current_offset
    }

    pub fn backspace_active_input(&mut self) {
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        if cursor == 0 {
            return;
        }
        let start = char_offset_to_byte_index(text.as_str(), cursor - 1);
        let end = char_offset_to_byte_index(text.as_str(), cursor);
        text.replace_range(start..end, "");
        *cursor_offset = Some(cursor - 1);
        self.update_composer_after_edit_if_needed(target);
    }

    pub fn delete_forward_active_input(&mut self) {
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        if cursor >= text.chars().count() {
            return;
        }
        let start = char_offset_to_byte_index(text.as_str(), cursor);
        let end = char_offset_to_byte_index(text.as_str(), cursor + 1);
        text.replace_range(start..end, "");
        *cursor_offset = Some(cursor);
        self.update_composer_after_edit_if_needed(target);
    }

    pub fn move_active_input_cursor_left(&mut self) {
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        *cursor_offset = Some(cursor.saturating_sub(1));
    }

    pub fn move_active_input_cursor_right(&mut self) {
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        let cursor = effective_cursor_offset(text.as_str(), *cursor_offset);
        *cursor_offset = Some((cursor + 1).min(text.chars().count()));
    }

    pub fn move_active_input_cursor_home(&mut self) {
        let target = self.active_text_input_target();
        let (_, cursor_offset) = self.text_and_cursor_mut(target);
        *cursor_offset = Some(0);
    }

    pub fn move_active_input_cursor_end(&mut self) {
        let target = self.active_text_input_target();
        let (text, cursor_offset) = self.text_and_cursor_mut(target);
        *cursor_offset = Some(text.chars().count());
    }

    pub fn move_composer_cursor_up(&mut self) {
        let cursor = self.composer_cursor_offset();
        let (row, column) = self.composer_visual_position_for_offset(cursor);
        if row == 0 {
            self.bottom_pane.input_cursor_offset = Some(0);
            return;
        }
        self.bottom_pane.input_cursor_offset =
            Some(self.composer_offset_for_visual_position(row - 1, column));
    }

    pub fn move_composer_cursor_down(&mut self) {
        let cursor = self.composer_cursor_offset();
        let (row, column) = self.composer_visual_position_for_offset(cursor);
        let target = self.composer_offset_for_visual_position(row + 1, column);
        self.bottom_pane.input_cursor_offset = Some(target);
    }
}
