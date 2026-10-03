//! Fixed-size identity key for the inputs read by active-turn assembly.

use crate::{
    agent::AgentExecutionMode,
    tui::{
        presentation_revision::PresentationRevision,
        state::{RuntimePhase, TuiApp},
        theme::ThemeRevision,
    },
};

#[derive(PartialEq, Eq)]
pub(super) struct ActivePrefixKey {
    turn: PresentationRevision,
    live: PresentationRevision,
    snapshot: PresentationRevision,
    detail: PresentationRevision,
    suggestion: PresentationRevision,
    pending_messages: PresentationRevision,
    queued_messages: PresentationRevision,
    has_thinking_stream: bool,
    thinking_visible: bool,
    has_response: bool,
    has_history: bool,
    busy: bool,
    phase: RuntimePhase,
    mode: AgentExecutionMode,
    thinking_collapsed: bool,
    approval_selection: usize,
    width: u16,
    theme: ThemeRevision,
}

impl ActivePrefixKey {
    pub(super) fn new(app: &TuiApp, width: u16, has_history: bool, thinking_visible: bool) -> Self {
        Self {
            turn: app.active_turn.revision(),
            live: app.active_live.revision(),
            snapshot: app.snapshot.revision(),
            detail: app.runtime_phase_detail.revision(),
            suggestion: app.bottom_pane.pending_planning_suggestion.revision(),
            pending_messages: app.bottom_pane.pending_follow_up_messages.revision(),
            queued_messages: app.bottom_pane.queued_follow_up_messages.revision(),
            has_thinking_stream: app.has_agent_thinking_stream(),
            thinking_visible,
            has_response: app.has_agent_stream(),
            has_history,
            busy: app.is_busy(),
            phase: app.runtime_phase,
            mode: app.agent_execution_mode,
            thinking_collapsed: app.thinking_collapsed,
            approval_selection: app.approval_picker_idx,
            width,
            theme: crate::tui::theme::revision(),
        }
    }
}

#[derive(PartialEq, Eq)]
pub(super) struct LiveThinkingKey {
    source: Option<PresentationRevision>,
    duration: Option<String>,
    width: u16,
    theme: ThemeRevision,
}

impl LiveThinkingKey {
    pub(super) fn new(app: &TuiApp, width: u16, duration: Option<String>) -> Self {
        Self {
            source: app
                .agent_thinking_stream
                .as_ref()
                .map(|stream| stream.presentation_revision()),
            duration,
            width,
            theme: crate::tui::theme::revision(),
        }
    }
}
