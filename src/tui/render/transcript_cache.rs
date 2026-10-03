//! Session-local committed and replaceable-tail layout caches.

use std::{path::Path, rc::Rc};

use ratatui::text::Line;

use super::{
    ResponseView,
    active_prefix::{ActivePrefixKey, LiveThinkingKey},
    cells::HistoryCell,
};
use super::{active_turn_cell, cells::ActiveTurnCell, committed_turn_lines, turn_divider_line};
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
    theme: crate::tui::theme::ThemeRevision,
}

#[derive(Default)]
struct CachedRows {
    width: u16,
    logical: Vec<Line<'static>>,
    block: Rc<RowBlock>,
}

impl CachedRows {
    fn update(&mut self, lines: Vec<Line<'static>>, width: u16, #[cfg(test)] work: &WorkMeter) {
        // Compare all styles after an input-key miss, retaining wrapped rows
        // when the changed inputs have no visible effect.
        if self.width != width || self.logical != lines {
            #[cfg(test)]
            work.record(WorkKind::Wrap, lines.len());
            let block = Rc::new(RowBlock::wrap(&lines, width));
            #[cfg(test)]
            work.record(WorkKind::Text, block.len());
            self.block = block;
            self.logical = lines;
            self.width = width;
        }
    }
}

#[derive(Default)]
pub(crate) struct CommittedTranscriptRenderCache {
    key: Option<HistoryKey>,
    rendered_turns: usize,
    history: SharedHistory,
    active: CachedRows,
    after_thinking: Option<CachedRows>,
    thinking: CachedRows,
    thinking_key: Option<LiveThinkingKey>,
    active_key: Option<ActivePrefixKey>,
    active_stream_view: Option<ResponseView>,
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
        self.active.update(
            active,
            width,
            #[cfg(test)]
            &self.work,
        );
    }
}

pub(super) fn materialize(app: &TuiApp, width: u16) -> TranscriptRows {
    materialize_cell(app, width, active_turn_cell(app))
}

fn materialize_cell(app: &TuiApp, width: u16, cell: ActiveTurnCell<'_>) -> TranscriptRows {
    let key = HistoryKey {
        generation: app.committed_render_generation,
        width,
        thinking_collapsed: app.thinking_collapsed,
        cwd: app.snapshot.cwd.clone(),
        theme: crate::tui::theme::revision(),
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

    let key = ActivePrefixKey::new(
        app,
        width,
        !cache.history.is_empty(),
        cell.has_visible_thinking(),
    );
    #[cfg(test)]
    let work = cache.work.clone();
    if cache.active_key.as_ref() != Some(&key) {
        let mut active = cell.shared_layout(width);
        if (!active.lines.is_empty() || active.after_thinking.is_some() || active.stream.is_some())
            && !cache.history.is_empty()
        {
            active.lines.insert(0, turn_divider_line(width));
        }
        cache.update_active(active.lines, width);
        if let Some(lines) = active.after_thinking {
            cache
                .after_thinking
                .get_or_insert_with(CachedRows::default)
                .update(
                    lines,
                    width,
                    #[cfg(test)]
                    &work,
                );
        } else {
            cache.after_thinking = None;
            cache.thinking = CachedRows::default();
            cache.thinking_key = None;
        }
        cache.active_stream_view = active.stream;
        cache.active_key = Some(key);
    }
    let mut rows = TranscriptRows::new(cache.history.clone(), cache.active.block.clone());
    if let Some(suffix) = cache
        .after_thinking
        .as_ref()
        .map(|suffix| suffix.block.clone())
    {
        let key = LiveThinkingKey::new(app, width, cell.thinking_duration_label());
        if cache.thinking_key.as_ref() != Some(&key) {
            let lines = cell
                .thinking_cell()
                .map(|thinking| thinking.display_lines(width))
                .unwrap_or_default();
            cache.thinking.update(
                lines,
                width,
                #[cfg(test)]
                &work,
            );
            cache.thinking_key = Some(key);
        }
        rows = TranscriptRows::joined(
            rows,
            TranscriptRows::new(SharedHistory::default(), cache.thinking.block.clone()),
        );
        rows = TranscriptRows::joined(rows, TranscriptRows::new(SharedHistory::default(), suffix));
    }
    if let Some(view) = cache.active_stream_view {
        let Some(stream) = &app.agent_markdown_stream else {
            debug_assert!(
                false,
                "shared response layout requires an active source stream"
            );
            return rows;
        };
        TranscriptRows::joined(rows, stream.response_rows(width, view))
    } else {
        rows
    }
}

#[cfg(test)]
#[path = "transcript_cache_key_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "active_prefix_time_tests.rs"]
mod time_tests;
