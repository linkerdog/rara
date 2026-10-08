use std::time::Duration;

use rara_persistence::redaction::redact_secrets;
use tokio::time::Instant;

use super::{SystemMessageKind, TuiApp};
use crate::tui::message_role::MessageRole;

const NOTICE_LIFETIME: Duration = Duration::from_secs(8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum NoticeLevel {
    Info,
    Warning,
    Error,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NoticeOwner {
    General,
    Paste,
}

enum NoticeRecord {
    Plain,
    MemoryOnly,
    Classified(SystemMessageKind),
}

#[derive(Debug)]
pub(crate) struct Notice {
    level: NoticeLevel,
    message: String,
    expires_at: Instant,
    owner: NoticeOwner,
}

impl Notice {
    pub(crate) fn level(&self) -> NoticeLevel {
        self.level
    }

    pub(crate) fn message(&self) -> &str {
        &self.message
    }
}

#[derive(Debug, Default)]
pub(super) struct NoticeState {
    current: Option<Notice>,
}

impl TuiApp {
    pub(crate) fn notice(&self) -> Option<&Notice> {
        self.notices.current.as_ref()
    }

    pub(crate) fn notice_text(&self) -> Option<&str> {
        self.notice().map(Notice::message)
    }

    pub(crate) fn push_notice(&mut self, level: NoticeLevel, message: impl Into<String>) {
        self.publish_notice(
            level,
            message.into(),
            NoticeOwner::General,
            NoticeRecord::Plain,
            Instant::now(),
        );
    }

    /// Surface storage failures without recursively queuing another write.
    pub(crate) fn push_unpersisted_notice(
        &mut self,
        level: NoticeLevel,
        message: impl Into<String>,
    ) {
        self.publish_notice(
            level,
            message.into(),
            NoticeOwner::General,
            NoticeRecord::MemoryOnly,
            Instant::now(),
        );
    }

    pub(crate) fn push_system_notice(
        &mut self,
        level: NoticeLevel,
        message: impl Into<String>,
        kind: SystemMessageKind,
    ) {
        self.publish_notice(
            level,
            message.into(),
            NoticeOwner::General,
            NoticeRecord::Classified(kind),
            Instant::now(),
        );
    }

    pub(super) fn push_paste_notice(&mut self, message: String) {
        self.publish_notice(
            NoticeLevel::Info,
            message,
            NoticeOwner::Paste,
            NoticeRecord::Plain,
            Instant::now(),
        );
    }

    fn publish_notice(
        &mut self,
        level: NoticeLevel,
        message: String,
        owner: NoticeOwner,
        record: NoticeRecord,
        now: Instant,
    ) {
        let message = redact_secrets(message);
        match record {
            NoticeRecord::MemoryOnly => self.active_turn.entries.push(super::TranscriptEntry::new(
                MessageRole::System,
                message.clone(),
            )),
            NoticeRecord::Plain => self.push_entry(MessageRole::System, message.clone()),
            NoticeRecord::Classified(kind) => self.push_system(message.clone(), kind),
        }
        self.notices.current = Some(Notice {
            level,
            message,
            expires_at: now + NOTICE_LIFETIME,
            owner,
        });
    }

    pub(crate) fn expire_notice(&mut self, now: Instant) -> bool {
        let expired = self.notice().is_some_and(|notice| now >= notice.expires_at);
        if expired {
            self.notices.current = None;
        }
        expired
    }

    pub(crate) fn clear_composer(&mut self) {
        self.bottom_pane.clear_input();
        if self
            .notice()
            .is_some_and(|notice| notice.owner == NoticeOwner::Paste)
        {
            self.notices.current = None;
        }
    }
}

#[cfg(test)]
#[path = "notices_tests.rs"]
mod tests;
