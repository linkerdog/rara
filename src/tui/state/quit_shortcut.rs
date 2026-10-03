use std::time::{Duration, Instant};

const CONFIRMATION_WINDOW: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitShortcutKey {
    CtrlC,
    CtrlD,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QuitShortcutAction {
    Armed,
    Confirmed,
}

#[derive(Default)]
pub(crate) struct QuitShortcutState {
    armed: Option<(QuitShortcutKey, Instant)>,
}

impl QuitShortcutState {
    pub(crate) fn press(&mut self, key: QuitShortcutKey, now: Instant) -> QuitShortcutAction {
        self.expire(now);
        if self.key() == Some(key) {
            self.clear();
            QuitShortcutAction::Confirmed
        } else {
            self.armed = Some((key, now));
            QuitShortcutAction::Armed
        }
    }

    pub(crate) fn key(&self) -> Option<QuitShortcutKey> {
        self.armed.map(|(key, _)| key)
    }

    pub(crate) fn clear(&mut self) {
        self.armed = None;
    }

    /// Return whether the footer needs repainting after this clock update.
    pub(crate) fn expire(&mut self, now: Instant) -> bool {
        if self.armed.is_some_and(|(_, armed_at)| {
            now.saturating_duration_since(armed_at) >= CONFIRMATION_WINDOW
        }) {
            self.clear();
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confirmation_requires_same_key_strictly_before_deadline() {
        let start = Instant::now();
        for key in [QuitShortcutKey::CtrlC, QuitShortcutKey::CtrlD] {
            let mut state = QuitShortcutState::default();
            assert_eq!(state.press(key, start), QuitShortcutAction::Armed);
            assert_eq!(
                state.press(key, start + CONFIRMATION_WINDOW - Duration::from_nanos(1)),
                QuitShortcutAction::Confirmed
            );
            assert_eq!(state.key(), None);
            assert_eq!(state.press(key, start), QuitShortcutAction::Armed);
            assert_eq!(
                state.press(key, start + CONFIRMATION_WINDOW),
                QuitShortcutAction::Armed
            );
        }
    }

    #[test]
    fn different_key_rearms_and_expiration_requests_one_redraw() {
        let start = Instant::now();
        let mut state = QuitShortcutState::default();
        state.press(QuitShortcutKey::CtrlC, start);
        let later = start + Duration::from_millis(500);
        assert_eq!(
            state.press(QuitShortcutKey::CtrlD, later),
            QuitShortcutAction::Armed
        );
        assert_eq!(state.key(), Some(QuitShortcutKey::CtrlD));
        assert!(!state.expire(start + CONFIRMATION_WINDOW));
        assert!(state.expire(later + CONFIRMATION_WINDOW));
        assert_eq!(state.key(), None);
        assert!(!state.expire(later + CONFIRMATION_WINDOW));
    }
}
