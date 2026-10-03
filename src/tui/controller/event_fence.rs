use std::collections::HashSet;

use crate::runtime_control::{RuntimeControlEvent, RuntimeEvent, SessionEvent};
use crate::tui::runtime::QueryTaskControl;
use crate::tui::runtime_port::accept_runtime_event;

#[derive(Default)]
pub(super) struct RuntimeEventFence {
    session_id: Option<String>,
    active_turn: Option<String>,
    terminal_turns: HashSet<String>,
}

impl RuntimeEventFence {
    pub(super) fn accept(
        &mut self,
        last_event: &mut Option<(Option<String>, u64, String)>,
        event: &RuntimeControlEvent,
        snapshot_session: &str,
        query: Option<&QueryTaskControl>,
    ) -> bool {
        let expected_session = query
            .map(|query| query.session_id.as_str())
            .or_else(|| (!snapshot_session.is_empty()).then_some(snapshot_session))
            .or(self.session_id.as_deref());
        if let Some(expected) = expected_session {
            if event
                .provenance
                .session_id
                .as_deref()
                .is_some_and(|id| id != expected)
                || (event.turn_id.is_some() && event.provenance.session_id.is_none())
            {
                return false;
            }
            if self.session_id.as_deref() != Some(expected) {
                self.session_id = Some(expected.to_owned());
                self.active_turn = None;
                self.terminal_turns.clear();
                *last_event = None;
            }
        }

        let started = matches!(
            event.event,
            RuntimeEvent::Session(SessionEvent::TurnStarted)
        );
        if let Some(turn) = event.turn_id.as_deref() {
            if query.is_some_and(|query| query.turn_id != turn)
                || self.terminal_turns.contains(turn)
            {
                return false;
            }
            if self.active_turn.as_deref() != Some(turn) && query.is_none() {
                let previous_active = self
                    .active_turn
                    .as_ref()
                    .is_some_and(|active| !self.terminal_turns.contains(active));
                if !started || previous_active {
                    return false;
                }
            }
        } else if (query.is_some() || self.active_turn.is_some())
            && matches!(
                event.event,
                RuntimeEvent::Assistant(_)
                    | RuntimeEvent::Tool(_)
                    | RuntimeEvent::Session(
                        SessionEvent::TurnStarted
                            | SessionEvent::TurnFinished { .. }
                            | SessionEvent::TurnFailed { .. }
                            | SessionEvent::TurnCancelled
                            | SessionEvent::TurnInterrupted
                    )
            )
        {
            return false;
        }

        let previous_sequence = last_event.as_ref().map_or(0, |(_, sequence, _)| *sequence);
        if event.sequence > 0 && event.sequence <= previous_sequence {
            return false;
        }
        if !accept_runtime_event(last_event, event) {
            return false;
        }
        if event.sequence == 0
            && let Some((_, sequence, _)) = last_event.as_mut()
        {
            *sequence = previous_sequence;
        }
        if let Some(turn) = &event.turn_id {
            self.session_id = event.provenance.session_id.clone();
            self.active_turn = Some(turn.clone());
            if is_terminal_turn_event(&event.event) {
                self.terminal_turns.insert(turn.clone());
                self.active_turn = None;
            }
        }
        true
    }

    pub(super) fn close(&mut self, query: &QueryTaskControl) {
        if self.session_id.as_deref() != Some(&query.session_id) {
            self.session_id = Some(query.session_id.clone());
            self.terminal_turns.clear();
        }
        self.active_turn = None;
        self.terminal_turns.insert(query.turn_id.clone());
    }
}

pub(super) fn is_terminal_turn_event(event: &RuntimeEvent) -> bool {
    matches!(
        event,
        RuntimeEvent::Session(
            SessionEvent::TurnFinished { .. }
                | SessionEvent::TurnFailed { .. }
                | SessionEvent::TurnCancelled
                | SessionEvent::TurnInterrupted
        )
    )
}
