use std::path::PathBuf;

use rara_persistence::redaction::redact_secrets;
use rara_state::state_db::PersistedTurnEntry;
use ratatui::text::Line;

use super::{
    PendingFollowUpMessage, RuntimePhase, SystemMessageKind, TranscriptEntry, TranscriptTurn,
    TuiApp,
};
use crate::tui::terminal_event::TerminalEvent;

fn is_agent_segment_boundary(entry: &TranscriptEntry) -> bool {
    if matches!(
        entry.role.as_str(),
        "Tool"
            | "Tool Result"
            | "Tool Error"
            | "Tool Progress"
            | "Thinking"
            | "Exploring"
            | "Planning"
            | "Running"
    ) {
        return true;
    }
    match &entry.payload {
        Some(crate::tui::state::TranscriptEntryPayload::Terminal(_)) => true,
        Some(crate::tui::state::TranscriptEntryPayload::Tool(payload)) => !payload.name.is_empty(),
        Some(crate::tui::state::TranscriptEntryPayload::Compaction(payload)) => payload.count > 0,
        _ => false,
    }
}

impl TuiApp {
    fn replace_current_agent_segment_message(turn: &mut TranscriptTurn, message: String) -> bool {
        let segment_start = turn
            .entries
            .iter()
            .rposition(is_agent_segment_boundary)
            .map_or(0, |idx| idx + 1);
        let Some(last_agent_idx) = turn.entries[segment_start..]
            .iter()
            .rposition(|entry| entry.role == "Agent")
            .map(|idx| segment_start + idx)
        else {
            return false;
        };

        turn.entries[last_agent_idx].message = message;
        let mut retained = Vec::with_capacity(turn.entries.len());
        for (idx, entry) in turn.entries.drain(..).enumerate() {
            if idx >= segment_start && idx != last_agent_idx && entry.role == "Agent" {
                continue;
            }
            retained.push(entry);
        }
        turn.entries = retained;
        true
    }

    fn reset_transcript_scroll_if_following_tail(&mut self) {
        // Keep the transcript pinned to the tail only when the user has not
        // manually scrolled upward. Once they scroll up, transcript mutations
        // should avoid yanking the viewport back to the bottom.
        if self.transcript_scroll == 0 {
            self.transcript_scroll = 0;
        }
    }

    pub fn push_entry(&mut self, role: &'static str, message: impl Into<String>) {
        let message = match role {
            "System" | "Runtime" => redact_secrets(message.into()),
            _ => message.into(),
        };
        if role == "You" && !self.active_turn.entries.is_empty() {
            self.commit_active_turn();
        }
        let entry = TranscriptEntry::new(role, message);
        self.record_entry_realtime(&PersistedTurnEntry {
            role: entry.role.clone(),
            message: entry.message.clone(),
        });
        self.active_turn.entries.push(entry);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn push_tool_entry(
        &mut self,
        call_id: Option<&str>,
        name: &str,
        status: super::ToolTranscriptStatus,
        message: impl Into<String>,
    ) {
        let entry = super::TranscriptEntry::tool(call_id, name, status, message);
        self.record_entry_realtime(&PersistedTurnEntry {
            role: entry.role.clone(),
            message: entry.message.clone(),
        });
        self.active_turn.entries.push(entry);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn push_compaction_entry(
        &mut self,
        count: usize,
        before_tokens: usize,
        after_tokens: usize,
        summary: impl Into<String>,
        recent_files: Vec<String>,
    ) {
        let entry = super::TranscriptEntry::compaction(
            count,
            before_tokens,
            after_tokens,
            summary,
            recent_files,
        );
        self.record_entry_realtime(&PersistedTurnEntry {
            role: entry.role.clone(),
            message: entry.message.clone(),
        });
        self.active_turn.entries.push(entry);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn push_terminal_event(&mut self, event: TerminalEvent) {
        let entry = TranscriptEntry::terminal_event(event);
        self.record_entry_realtime(&PersistedTurnEntry {
            role: entry.role.clone(),
            message: entry.message.clone(),
        });
        self.active_turn.entries.push(entry);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn push_system(&mut self, message: impl Into<String>, kind: SystemMessageKind) {
        let entry = TranscriptEntry::system(redact_secrets(message.into()), kind);
        self.record_entry_realtime(&PersistedTurnEntry {
            role: entry.role.clone(),
            message: entry.message.clone(),
        });
        self.active_turn.entries.push(entry);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn append_agent_delta(&mut self, delta: &str) {
        self.finalize_agent_thinking_stream();
        let cwd = if !self.snapshot.cwd.is_empty() {
            PathBuf::from(self.snapshot.cwd.as_str())
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let stream = self
            .agent_markdown_stream
            .get_or_insert_with(|| super::AgentMarkdownStreamState::new(cwd));
        stream.push_delta(delta);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn append_agent_thinking_delta(&mut self, delta: &str) {
        if self.agent_markdown_stream.is_some() {
            self.finalize_agent_stream(None);
        }
        let cwd = if !self.snapshot.cwd.is_empty() {
            PathBuf::from(self.snapshot.cwd.as_str())
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let is_first_delta = self.agent_thinking_stream.is_none();
        let stream = self
            .agent_thinking_stream
            .get_or_insert_with(|| super::AgentMarkdownStreamState::new(cwd));
        stream.push_delta(delta);
        if is_first_delta {
            self.active_live.thinking_started_at = Some(std::time::Instant::now());
        }
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn finalize_agent_thinking_stream(&mut self) {
        let Some(stream) = self.agent_thinking_stream.take() else {
            return;
        };
        if stream.raw_text.trim().is_empty() {
            return;
        }
        let message = stream.sanitized_raw_text().trim_end().to_string();
        if message.trim().is_empty() {
            return;
        }
        self.push_active_progress_entry("Thinking", message);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn agent_stream_lines(&self) -> Option<&[Line<'static>]> {
        self.agent_markdown_stream
            .as_ref()
            .map(|stream| stream.display_lines.as_slice())
    }

    pub fn agent_thinking_stream_lines(&self) -> Option<&[Line<'static>]> {
        self.agent_thinking_stream
            .as_ref()
            .map(|stream| stream.display_lines.as_slice())
    }

    pub fn has_agent_stream(&self) -> bool {
        self.agent_markdown_stream.is_some()
    }

    pub fn has_agent_thinking_stream(&self) -> bool {
        self.agent_thinking_stream.is_some()
    }

    pub fn finalize_agent_stream(&mut self, final_message: Option<String>) {
        self.finalize_agent_thinking_stream();
        let fallback = self
            .agent_markdown_stream
            .take()
            .map(|mut stream| {
                stream.finalize_display_lines();
                stream.sanitized_raw_text()
            })
            .filter(|text| !text.is_empty());
        let Some(message) = final_message.or(fallback) else {
            return;
        };
        let message = crate::tui::display_sanitize::sanitize_display_text(
            &crate::control_tokens::scrub_internal_control_tokens(&message),
        );
        if message.trim().is_empty() {
            return;
        }

        if Self::replace_current_agent_segment_message(&mut self.active_turn, message.clone()) {
            self.replace_live_log_entries(&self.active_turn.entries);
            self.reset_transcript_scroll_if_following_tail();
            return;
        }
        if self.active_turn.entries.is_empty()
            && let Some(turn) = self.committed_turns.last_mut()
            && Self::replace_current_agent_segment_message(turn, message.clone())
        {
            self.invalidate_committed_render_cache();
            self.reset_transcript_scroll_if_following_tail();
            return;
        }
        self.push_entry("Agent", message);
    }

    pub fn push_notice(&mut self, message: impl Into<String>) {
        let message = redact_secrets(message.into());
        self.bottom_pane.notice = Some(message.clone());
        self.push_entry("System", message);
    }

    pub fn reset_transcript(&mut self) {
        self.committed_turns.clear();
        self.active_turn.entries.clear();
        self.clear_live_log();
        self.invalidate_committed_render_cache();
        self.transcript_scroll = 0;
        self.agent_markdown_stream = None;
        self.agent_thinking_stream = None;
        self.clear_active_live_sections();
        self.bottom_pane.pending_planning_suggestion = None;
        self.bottom_pane.pending_follow_up_messages.clear();
        self.bottom_pane.queued_follow_up_messages.clear();
        self.running_tool_boundary_count = 0;
        self.clear_pending_plan_approval();
        self.bottom_pane.notice = Some("Cleared local transcript view.".into());
    }

    pub fn scroll_transcript(&mut self, delta: i32) {
        if delta < 0 {
            self.transcript_scroll = self
                .transcript_scroll
                .saturating_add(delta.unsigned_abs() as usize);
        } else {
            self.transcript_scroll = self.transcript_scroll.saturating_sub(delta as usize);
        }
    }

    pub fn scroll_context(&mut self, delta: i32) {
        if delta > 0 {
            self.context_scroll = self.context_scroll.saturating_add(delta as u16);
        } else {
            self.context_scroll = self
                .context_scroll
                .saturating_sub(delta.unsigned_abs() as u16);
        }
    }

    pub fn set_runtime_phase(&mut self, phase: RuntimePhase, detail: Option<String>) {
        self.runtime_phase = phase;
        self.runtime_phase_detail = detail;
    }

    pub fn runtime_phase_label(&self) -> &'static str {
        match self.runtime_phase {
            RuntimePhase::Idle => "idle",
            RuntimePhase::LocalCommand => "local-command",
            RuntimePhase::SendingPrompt => "sending-prompt",
            RuntimePhase::ProcessingResponse => "processing-response",
            RuntimePhase::RunningTool => "running-tool",
            RuntimePhase::RebuildingBackend => "rebuilding-backend",
            RuntimePhase::BackendReady => "backend-ready",
            RuntimePhase::OAuthStarting => "oauth-starting",
            RuntimePhase::OAuthWaitingCallback => "oauth-waiting-callback",
            RuntimePhase::OAuthExchangingToken => "oauth-exchanging-token",
            RuntimePhase::OAuthDeviceCodePrompt => "oauth-device-code-prompt",
            RuntimePhase::OAuthPollingDeviceCode => "oauth-polling-device-code",
            RuntimePhase::OAuthVerifying => "oauth-verifying",
            RuntimePhase::OAuthSuccess => "oauth-success",
            RuntimePhase::OAuthSaved => "oauth-saved",
            RuntimePhase::OAuthError => "oauth-error",
            RuntimePhase::Failed => "failed",
        }
    }

    pub fn remember_command(&mut self, command_name: &str) {
        self.recent_commands.retain(|value| value != command_name);
        self.recent_commands.insert(0, command_name.to_string());
        self.recent_commands.truncate(5);
    }

    pub fn has_any_transcript(&self) -> bool {
        !self.committed_turns.is_empty() || !self.active_turn.entries.is_empty()
    }

    pub fn transcript_entry_count(&self) -> usize {
        self.committed_turns
            .iter()
            .map(|turn| turn.entries.len())
            .sum::<usize>()
            + self.active_turn.entries.len()
    }

    fn commit_active_turn(&mut self) {
        self.finalize_agent_stream(None);
        if self.active_turn.entries.is_empty() {
            self.clear_active_live_sections();
            return;
        }
        self.active_turn.thinking_duration =
            self.active_live.thinking_started_at.map(|s| s.elapsed());
        let ordinal = self.committed_turns.len();
        let turn_to_persist = self.active_turn.clone();
        if !self.persist_turn(ordinal, &turn_to_persist) {
            self.clear_active_live_sections();
            return;
        }
        let turn = std::mem::take(&mut self.active_turn);
        self.committed_turns.push(turn);
        self.clear_live_log();
        self.invalidate_committed_render_cache();
        self.reset_transcript_scroll_if_following_tail();
        self.clear_active_live_sections();
    }

    pub fn finalize_active_turn(&mut self) {
        self.commit_active_turn();
    }

    pub fn restore_committed_turns(&mut self, turns: Vec<TranscriptTurn>) {
        self.committed_turns = turns;
        self.active_turn.entries.clear();
        self.clear_live_log();
        self.invalidate_committed_render_cache();
        self.transcript_scroll = 0;
        self.agent_markdown_stream = None;
        self.agent_thinking_stream = None;
        self.clear_active_live_sections();
    }

    pub(crate) fn invalidate_committed_render_cache(&mut self) {
        self.committed_render_generation = self.committed_render_generation.wrapping_add(1);
        *self.committed_render_cache.borrow_mut() =
            super::CommittedTranscriptRenderCache::default();
    }

    pub fn clear_active_live_sections(&mut self) {
        self.active_live = super::ActiveLiveSections::default();
    }

    fn push_active_progress_entry(&mut self, role: &'static str, message: String) {
        let message = crate::tui::display_sanitize::sanitize_display_text(&message);
        self.push_entry(role, message);
    }

    #[cfg(test)]
    pub fn record_exploration_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        self.cache_exploration_action(action.clone());
        self.push_active_progress_entry("Exploring", action);
    }

    pub(crate) fn cache_exploration_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        if !self
            .active_live
            .exploration_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.exploration_actions.push(action);
        }
    }

    pub fn record_exploration_note(&mut self, note: impl Into<String>) {
        let note = crate::tui::display_sanitize::sanitize_display_text(&note.into());
        if !self
            .active_live
            .exploration_notes
            .iter()
            .any(|item| item == &note)
        {
            self.active_live.exploration_notes.push(note.clone());
        }
        self.push_active_progress_entry("Exploring", note);
    }

    #[cfg(test)]
    pub fn record_running_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        self.cache_running_action(action.clone());
        self.push_active_progress_entry("Running", action);
    }

    pub(crate) fn cache_running_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        if !self
            .active_live
            .running_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.running_actions.push(action);
        }
    }

    #[cfg(test)]
    pub fn record_planning_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        self.cache_planning_action(action.clone());
        self.push_active_progress_entry("Planning", action);
    }

    pub(crate) fn cache_planning_action(&mut self, action: impl Into<String>) {
        let action = crate::tui::display_sanitize::sanitize_display_text(&action.into());
        if !self
            .active_live
            .planning_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.planning_actions.push(action);
        }
    }

    pub fn record_planning_note(&mut self, note: impl Into<String>) {
        let note = crate::tui::display_sanitize::sanitize_display_text(&note.into());
        if !self
            .active_live
            .planning_notes
            .iter()
            .any(|item| item == &note)
        {
            self.active_live.planning_notes.push(note.clone());
        }
        self.push_active_progress_entry("Planning", note);
    }

    pub fn has_pending_planning_suggestion(&self) -> bool {
        self.bottom_pane.pending_planning_suggestion.is_some()
    }

    pub fn has_queued_follow_up_messages(&self) -> bool {
        !self.bottom_pane.pending_follow_up_messages.is_empty()
            || !self.bottom_pane.queued_follow_up_messages.is_empty()
    }

    pub fn queued_follow_up_count(&self) -> usize {
        self.bottom_pane.pending_follow_up_messages.len()
            + self.bottom_pane.queued_follow_up_messages.len()
    }

    pub fn has_pending_follow_up_messages(&self) -> bool {
        !self.bottom_pane.pending_follow_up_messages.is_empty()
    }

    pub fn pending_follow_up_count(&self) -> usize {
        self.bottom_pane.pending_follow_up_messages.len()
    }

    #[cfg(test)]
    pub fn queued_follow_up_preview(&self) -> Option<&str> {
        self.bottom_pane
            .pending_follow_up_messages
            .first()
            .map(|item| item.text.as_str())
            .or_else(|| {
                self.bottom_pane
                    .queued_follow_up_messages
                    .first()
                    .map(String::as_str)
            })
    }

    pub fn pending_follow_up_preview(&self) -> Option<&str> {
        self.bottom_pane
            .pending_follow_up_messages
            .first()
            .map(|item| item.text.as_str())
    }

    pub fn queued_end_of_turn_preview(&self) -> Option<&str> {
        self.bottom_pane
            .queued_follow_up_messages
            .first()
            .map(String::as_str)
    }

    pub fn queue_follow_up_message(&mut self, message: impl Into<String>) -> usize {
        let message = message.into();
        if !message.trim().is_empty() {
            self.bottom_pane.queued_follow_up_messages.push(message);
        }
        self.queued_follow_up_count()
    }

    pub fn queue_follow_up_message_after_next_tool_boundary(
        &mut self,
        message: impl Into<String>,
    ) -> usize {
        let message = message.into();
        if !message.trim().is_empty() {
            self.bottom_pane
                .pending_follow_up_messages
                .push(PendingFollowUpMessage {
                    text: message,
                    release_after_boundary: self.running_tool_boundary_count.saturating_add(1),
                });
        }
        self.queued_follow_up_count()
    }

    #[cfg(test)]
    pub fn pop_queued_follow_up_message(&mut self) -> Option<String> {
        if self.bottom_pane.queued_follow_up_messages.is_empty() {
            None
        } else {
            Some(self.bottom_pane.queued_follow_up_messages.remove(0))
        }
    }

    pub fn drain_queued_follow_up_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.bottom_pane.queued_follow_up_messages)
    }

    pub fn begin_running_turn(&mut self) {
        self.running_tool_boundary_count = 0;
    }

    pub fn release_pending_follow_ups(&mut self) {
        if self.bottom_pane.pending_follow_up_messages.is_empty() {
            return;
        }
        let released = self
            .bottom_pane
            .pending_follow_up_messages
            .drain(..)
            .map(|item| item.text)
            .collect::<Vec<_>>();
        self.bottom_pane.queued_follow_up_messages.extend(released);
    }

    pub fn advance_running_tool_boundary(&mut self) {
        self.running_tool_boundary_count = self.running_tool_boundary_count.saturating_add(1);
        if self.bottom_pane.pending_follow_up_messages.is_empty() {
            return;
        }
        let current = self.running_tool_boundary_count;
        let mut still_pending = Vec::new();
        let mut released = Vec::new();
        for item in self.bottom_pane.pending_follow_up_messages.drain(..) {
            if item.release_after_boundary <= current {
                released.push(item.text);
            } else {
                still_pending.push(item);
            }
        }
        self.bottom_pane.pending_follow_up_messages = still_pending;
        self.bottom_pane.queued_follow_up_messages.extend(released);
    }

    #[cfg(test)]
    pub fn queue_planning_suggestion(&mut self, prompt: impl Into<String>) {
        self.bottom_pane.pending_planning_suggestion = Some(prompt.into());
        self.bottom_pane.notice = Some(
            "This looks like a non-trivial task. Enter planning mode first or continue in execute mode."
                .into(),
        );
        self.transcript_scroll = 0;
    }

    pub fn clear_pending_planning_suggestion(&mut self) {
        self.bottom_pane.pending_planning_suggestion = None;
    }
}
