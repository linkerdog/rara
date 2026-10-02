//! Session-local committed and replaceable-tail layout caches.

use std::{path::Path, rc::Rc};

use ratatui::text::Line;

use super::{ActiveCell, active_turn_cell, committed_turn_lines, turn_divider_line};
#[cfg(test)]
use crate::tui::transcript_work::{WorkKind, WorkMeter};
use crate::tui::{
    state::TuiApp,
    transcript_rows::{RowBlock, SharedHistory, TranscriptRows},
};

#[derive(Debug, PartialEq, Eq)]
struct HistoryKey {
    generation: u64,
    width: u16,
    thinking_collapsed: bool,
    cwd: String,
}

#[derive(Default)]
pub(crate) struct CommittedTranscriptRenderCache {
    key: Option<HistoryKey>,
    rendered_turns: usize,
    history: SharedHistory,
    active_width: u16,
    active_logical: Vec<Line<'static>>,
    active: Rc<RowBlock>,
    #[cfg(test)]
    pub(crate) work: WorkMeter,
}

impl CommittedTranscriptRenderCache {
    pub(crate) fn invalidate_history(&mut self) {
        self.key = None;
        self.rendered_turns = 0;
        self.history = SharedHistory::default();
    }

    fn update_active(&mut self, active: Vec<Line<'static>>, width: u16) {
        // Presentation writes are not yet encapsulated by exhaustive revisions.
        // Compare every styled logical row, never only edges/counts/text lengths.
        if self.active_width != width || self.active_logical != active {
            #[cfg(test)]
            self.work.record(WorkKind::Wrap, active.len());
            let block = Rc::new(RowBlock::wrap(&active, width));
            #[cfg(test)]
            self.work.record(WorkKind::Text, block.len());
            self.active = block;
            self.active_logical = active;
            self.active_width = width;
        }
    }
}

pub(super) fn materialize(app: &TuiApp, width: u16) -> TranscriptRows {
    let key = HistoryKey {
        generation: app.committed_render_generation,
        width,
        thinking_collapsed: app.thinking_collapsed,
        cwd: app.snapshot.cwd.clone(),
    };
    let mut cache = app.committed_render_cache.borrow_mut();
    if cache.key.as_ref() != Some(&key) || cache.rendered_turns > app.committed_turns.len() {
        cache.invalidate_history();
        cache.key = Some(key);
    }
    let cwd = (!app.snapshot.cwd.is_empty()).then(|| Path::new(app.snapshot.cwd.as_str()));
    for turn in &app.committed_turns[cache.rendered_turns..] {
        let mut lines = committed_turn_lines(
            &turn.entries,
            cwd,
            width,
            app.thinking_collapsed,
            turn.thinking_duration,
        );
        if lines.is_empty() {
            continue;
        }
        if !cache.history.is_empty() {
            lines.insert(0, turn_divider_line(width));
        }
        #[cfg(test)]
        cache.work.record(WorkKind::Wrap, lines.len());
        let block = Rc::new(RowBlock::wrap(&lines, width));
        #[cfg(test)]
        cache.work.record(WorkKind::Text, block.len());
        cache.history.append(block);
    }
    cache.rendered_turns = app.committed_turns.len();

    let mut active = active_turn_cell(app).display_lines(width);
    if !active.is_empty() && !cache.history.is_empty() {
        active.insert(0, turn_divider_line(width));
    }
    cache.update_active(active, width);
    TranscriptRows::new(cache.history.clone(), cache.active.clone())
}

#[cfg(test)]
#[path = "transcript_cache_key_tests.rs"]
mod tests;
