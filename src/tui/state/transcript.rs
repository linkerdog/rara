use std::path::PathBuf;

use ratatui::text::Line;

use super::{
    ActiveLiveEvent, ActiveLiveSections, PendingFollowUpMessage, RuntimePhase, TranscriptEntry,
    TranscriptTurn, TuiApp,
};
use crate::redaction::redact_secrets;
use crate::tui::terminal_event::TerminalEvent;

fn legacy_active_live_sections(live: &ActiveLiveSections) -> Vec<(&'static str, Vec<String>)> {
    vec![
        (
            "Exploring",
            live.exploration_actions
                .iter()
                .chain(live.exploration_notes.iter())
                .cloned()
                .collect::<Vec<_>>(),
        ),
        (
            "Planning",
            live.planning_actions
                .iter()
                .chain(live.planning_notes.iter())
                .cloned()
                .collect::<Vec<_>>(),
        ),
        ("Running", live.running_actions.to_vec()),
    ]
}

impl TuiApp {
    fn replace_turn_agent_message(turn: &mut TranscriptTurn, message: String) -> bool {
        let Some(last_agent_idx) = turn.entries.iter().rposition(|entry| entry.role == "Agent")
        else {
            return false;
        };

        turn.entries[last_agent_idx].message = message;

        let mut retained = Vec::with_capacity(turn.entries.len());
        for (idx, entry) in turn.entries.drain(..).enumerate() {
            if entry.role == "Agent" && idx != last_agent_idx {
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
        self.active_turn
            .entries
            .push(TranscriptEntry::new(role, message));
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn push_terminal_event(&mut self, event: TerminalEvent) {
        self.active_turn
            .entries
            .push(TranscriptEntry::terminal_event(event));
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn append_to_latest_entry(&mut self, role: &'static str, delta: &str) {
        if let Some(last) = self.active_turn.entries.last_mut() {
            if last.role == role {
                last.message.push_str(delta);
                self.reset_transcript_scroll_if_following_tail();
                return;
            }
        }
        self.push_entry(role, delta.to_string());
    }

    pub fn append_agent_delta(&mut self, delta: &str) {
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
        let cwd = if !self.snapshot.cwd.is_empty() {
            PathBuf::from(self.snapshot.cwd.as_str())
        } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
        };
        let stream = self
            .agent_thinking_stream
            .get_or_insert_with(|| super::AgentMarkdownStreamState::new(cwd));
        stream.push_delta(delta);
        self.reset_transcript_scroll_if_following_tail();
    }

    pub fn flush_agent_thinking_stream_to_live_event(&mut self) {
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
        self.push_active_live_event(ActiveLiveEvent::Thinking(message));
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
        self.flush_agent_thinking_stream_to_live_event();
        let fallback = self
            .agent_markdown_stream
            .take()
            .map(|stream| stream.sanitized_raw_text())
            .filter(|text| !text.is_empty());
        let Some(message) = final_message.or(fallback) else {
            return;
        };
        let message = crate::control_tokens::scrub_internal_control_tokens(&message);
        if message.trim().is_empty() {
            return;
        }

        if Self::replace_turn_agent_message(&mut self.active_turn, message.clone()) {
            self.reset_transcript_scroll_if_following_tail();
            return;
        }
        if self.active_turn.entries.is_empty() {
            if let Some(turn) = self.committed_turns.last_mut() {
                if Self::replace_turn_agent_message(turn, message.clone()) {
                    self.invalidate_committed_render_cache();
                    self.reset_transcript_scroll_if_following_tail();
                    return;
                }
            }
        }
        self.push_entry("Agent", message);
    }

    pub fn push_notice(&mut self, message: impl Into<String>) {
        let message = redact_secrets(message.into());
        self.notice = Some(message.clone());
        self.push_entry("System", message);
    }

    pub fn reset_transcript(&mut self) {
        self.committed_turns.clear();
        self.active_turn.entries.clear();
        self.inserted_turns = 0;
        self.invalidate_committed_render_cache();
        self.transcript_scroll = 0;
        self.agent_markdown_stream = None;
        self.agent_thinking_stream = None;
        self.clear_active_live_sections();
        self.pending_planning_suggestion = None;
        self.pending_follow_up_messages.clear();
        self.queued_follow_up_messages.clear();
        self.running_tool_boundary_count = 0;
        self.set_plan_approval_interaction(false);
        self.notice = Some("Cleared local transcript view.".into());
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
            RuntimePhase::OAuthSaved => "oauth-saved",
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

    pub fn committed_entry_count(&self) -> usize {
        self.committed_turns
            .iter()
            .map(|turn| turn.entries.len())
            .sum()
    }

    fn materialize_active_live_entries(&mut self) {
        if !self.active_live.events.is_empty() {
            for event in &self.active_live.events {
                self.active_turn.entries.push(TranscriptEntry::new(
                    event.role(),
                    event.message().to_string(),
                ));
            }
            return;
        }

        let sections = legacy_active_live_sections(&self.active_live);

        for (role, lines) in sections {
            if lines.is_empty() {
                continue;
            }
            self.active_turn
                .entries
                .push(TranscriptEntry::new(role, lines.join("\n")));
        }
    }

    fn commit_active_turn(&mut self) {
        self.finalize_agent_stream(None);
        self.materialize_active_live_entries();
        if self.active_turn.entries.is_empty() {
            self.clear_active_live_sections();
            return;
        }
        let turn = std::mem::take(&mut self.active_turn);
        let ordinal = self.committed_turns.len();
        self.persist_turn(ordinal, &turn);
        self.committed_turns.push(turn);
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
        self.inserted_turns = 0;
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

    fn push_active_live_event(&mut self, event: ActiveLiveEvent) {
        self.active_live.events.push(event);
    }

    pub fn record_exploration_action(&mut self, action: impl Into<String>) {
        let action = action.into();
        if !self
            .active_live
            .exploration_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.exploration_actions.push(action.clone());
        }
        self.push_active_live_event(ActiveLiveEvent::ExplorationAction(action));
    }

    pub fn record_exploration_note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self
            .active_live
            .exploration_notes
            .iter()
            .any(|item| item == &note)
        {
            self.active_live.exploration_notes.push(note.clone());
        }
        self.push_active_live_event(ActiveLiveEvent::ExplorationNote(note));
    }

    pub fn record_running_action(&mut self, action: impl Into<String>) {
        let action = action.into();
        if !self
            .active_live
            .running_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.running_actions.push(action.clone());
        }
        self.push_active_live_event(ActiveLiveEvent::RunningAction(action));
    }

    pub fn record_planning_action(&mut self, action: impl Into<String>) {
        let action = action.into();
        if !self
            .active_live
            .planning_actions
            .iter()
            .any(|item| item == &action)
        {
            self.active_live.planning_actions.push(action.clone());
        }
        self.push_active_live_event(ActiveLiveEvent::PlanningAction(action));
    }

    pub fn record_planning_note(&mut self, note: impl Into<String>) {
        let note = note.into();
        if !self
            .active_live
            .planning_notes
            .iter()
            .any(|item| item == &note)
        {
            self.active_live.planning_notes.push(note.clone());
        }
        self.push_active_live_event(ActiveLiveEvent::PlanningNote(note));
    }

    pub fn has_pending_planning_suggestion(&self) -> bool {
        self.pending_planning_suggestion.is_some()
    }

    pub fn has_queued_follow_up_messages(&self) -> bool {
        !self.pending_follow_up_messages.is_empty() || !self.queued_follow_up_messages.is_empty()
    }

    pub fn queued_follow_up_count(&self) -> usize {
        self.pending_follow_up_messages.len() + self.queued_follow_up_messages.len()
    }

    pub fn has_pending_follow_up_messages(&self) -> bool {
        !self.pending_follow_up_messages.is_empty()
    }

    pub fn pending_follow_up_count(&self) -> usize {
        self.pending_follow_up_messages.len()
    }

    pub fn queued_follow_up_preview(&self) -> Option<&str> {
        self.pending_follow_up_messages
            .first()
            .map(|item| item.text.as_str())
            .or_else(|| self.queued_follow_up_messages.first().map(String::as_str))
    }

    pub fn pending_follow_up_preview(&self) -> Option<&str> {
        self.pending_follow_up_messages
            .first()
            .map(|item| item.text.as_str())
    }

    pub fn queued_end_of_turn_preview(&self) -> Option<&str> {
        self.queued_follow_up_messages.first().map(String::as_str)
    }

    pub fn queue_follow_up_message(&mut self, message: impl Into<String>) -> usize {
        let message = message.into();
        if !message.trim().is_empty() {
            self.queued_follow_up_messages.push(message);
        }
        self.queued_follow_up_count()
    }

    pub fn queue_follow_up_message_after_next_tool_boundary(
        &mut self,
        message: impl Into<String>,
    ) -> usize {
        let message = message.into();
        if !message.trim().is_empty() {
            self.pending_follow_up_messages
                .push(PendingFollowUpMessage {
                    text: message,
                    release_after_boundary: self.running_tool_boundary_count.saturating_add(1),
                });
        }
        self.queued_follow_up_count()
    }

    pub fn pop_queued_follow_up_message(&mut self) -> Option<String> {
        if self.queued_follow_up_messages.is_empty() {
            None
        } else {
            Some(self.queued_follow_up_messages.remove(0))
        }
    }

    pub fn drain_queued_follow_up_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.queued_follow_up_messages)
    }

    pub fn begin_running_turn(&mut self) {
        self.running_tool_boundary_count = 0;
    }

    pub fn release_pending_follow_ups(&mut self) {
        if self.pending_follow_up_messages.is_empty() {
            return;
        }
        let released = self
            .pending_follow_up_messages
            .drain(..)
            .map(|item| item.text)
            .collect::<Vec<_>>();
        self.queued_follow_up_messages.extend(released);
    }

    pub fn advance_running_tool_boundary(&mut self) {
        self.running_tool_boundary_count = self.running_tool_boundary_count.saturating_add(1);
        if self.pending_follow_up_messages.is_empty() {
            return;
        }
        let current = self.running_tool_boundary_count;
        let mut still_pending = Vec::new();
        let mut released = Vec::new();
        for item in self.pending_follow_up_messages.drain(..) {
            if item.release_after_boundary <= current {
                released.push(item.text);
            } else {
                still_pending.push(item);
            }
        }
        self.pending_follow_up_messages = still_pending;
        self.queued_follow_up_messages.extend(released);
    }

    pub fn queue_planning_suggestion(&mut self, prompt: impl Into<String>) {
        self.pending_planning_suggestion = Some(prompt.into());
        self.notice = Some(
            "This looks like a non-trivial task. Enter planning mode first or continue in execute mode."
                .into(),
        );
        self.transcript_scroll = 0;
    }

    pub fn take_pending_planning_suggestion(&mut self) -> Option<String> {
        self.pending_planning_suggestion.take()
    }

    pub fn clear_pending_planning_suggestion(&mut self) {
        self.pending_planning_suggestion = None;
    }
}
