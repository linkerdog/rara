use super::{NoticeLevel, SystemMessageKind, TuiApp};

impl TuiApp {
    pub(crate) fn poll_diagnostics(&mut self) -> bool {
        let Some(reader) = self.diagnostics.as_ref() else {
            return false;
        };
        let records = reader.drain();
        let mut changed = false;
        for record in records {
            let level = match record.level {
                log::Level::Error => NoticeLevel::Error,
                log::Level::Warn => NoticeLevel::Warning,
                log::Level::Info | log::Level::Debug | log::Level::Trace => NoticeLevel::Info,
            };
            // Recovery paths can log a diagnostic and include it in one fuller
            // notice. Keep that notice instead of recording its fragment again.
            if self.notice().is_some_and(|notice| {
                (notice.level() == level || notice.level() == NoticeLevel::Error)
                    && notice.message().contains(&record.message)
            }) {
                continue;
            }
            self.push_system_notice(level, record.message, SystemMessageKind::Other);
            changed = true;
        }
        changed
    }
}
