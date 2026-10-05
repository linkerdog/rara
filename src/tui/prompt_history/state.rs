use std::collections::{HashSet, VecDeque};

use anyhow::{Context, Result};
use rara_persistence::prompt_history::{
    MAX_HISTORY_ENTRIES, PromptHistoryEntry, PromptHistoryStore,
};
use tokio::sync::oneshot;

use super::worker::{HistoryIo, HistoryRead};
use crate::tui::state::{NoticeLevel, Overlay, TuiApp};

#[derive(Clone, Copy, Debug)]
pub(crate) enum HistoryAction {
    Open,
    Older,
    Newer,
    Accept,
}

struct LocalEntry {
    entry: PromptHistoryEntry,
    revision: u64,
    queued: bool,
}

struct LoadRequest {
    receiver: oneshot::Receiver<Result<HistoryRead>>,
    revision: u64,
}

struct PendingNavigation {
    input: String,
    cursor: Option<usize>,
    overlay: Option<Overlay>,
    steps: usize,
}

#[derive(Default)]
pub(crate) struct HistoryState {
    pub query: String,
    pub query_cursor: Option<usize>,
    pub selected: usize,
    selection_anchor: Option<String>,
    store: Option<PromptHistoryStore>,
    io: Option<HistoryIo>,
    local: VecDeque<LocalEntry>,
    revision: u64,
    status_revision: u64,
    load: Option<LoadRequest>,
    navigation: Option<PendingNavigation>,
}

impl HistoryState {
    pub(crate) fn reset_selection(&mut self) {
        self.selected = 0;
        self.selection_anchor = None;
    }

    fn io(&mut self) -> Result<Option<&HistoryIo>> {
        if self.io.is_none()
            && let Some(store) = &self.store
        {
            self.io = Some(HistoryIo::start(store.clone())?);
        }
        Ok(self.io.as_ref())
    }

    fn request_load(&mut self) -> Result<()> {
        if self.load.is_some() {
            return Ok(());
        }
        if let Some(io) = self.io()? {
            let receiver = io.load()?;
            self.load = Some(LoadRequest {
                receiver,
                revision: self.revision,
            });
        }
        Ok(())
    }
}

impl TuiApp {
    /// Bind the real TUI to its config home; constructors and render harnesses stay I/O-free.
    pub(crate) fn attach_prompt_history(&mut self) {
        if self.config.tui.history.enabled {
            let home = self
                .config_manager
                .path
                .parent()
                .unwrap_or_else(|| std::path::Path::new("."));
            self.prompt_history.store = Some(PromptHistoryStore::new(home));
        }
    }

    pub fn record_input_history(&mut self, input: &str) {
        let Some(entry) = PromptHistoryEntry::new(input) else {
            self.reset_input_history_navigation();
            return;
        };
        if self
            .input_history
            .last()
            .is_some_and(|previous| previous == entry.text())
        {
            self.reset_input_history_navigation();
            return;
        }
        let queued = if self.config.tui.history.enabled {
            match self
                .prompt_history
                .io()
                .and_then(|io| io.map(|io| io.append(entry.clone())).transpose())
            {
                Ok(result) => result.is_some(),
                Err(error) => {
                    self.push_notice(
                        NoticeLevel::Warning,
                        format!("Prompt history was not saved: {error:#}"),
                    );
                    false
                }
            }
        } else {
            false
        };
        self.prompt_history.revision = self.prompt_history.revision.wrapping_add(1);
        self.prompt_history.local.push_back(LocalEntry {
            entry: entry.clone(),
            revision: self.prompt_history.revision,
            queued,
        });
        if self.prompt_history.local.len() > MAX_HISTORY_ENTRIES {
            self.prompt_history.local.pop_front();
        }
        self.input_history.push(entry.text().to_owned());
        if self.input_history.len() > MAX_HISTORY_ENTRIES {
            self.input_history
                .drain(..self.input_history.len() - MAX_HISTORY_ENTRIES);
        }
        self.reset_input_history_navigation();
    }

    pub(crate) fn prompt_history_can_load(&self) -> bool {
        self.config.tui.history.enabled && self.prompt_history.store.is_some()
    }

    pub(crate) fn prompt_history_loading(&self) -> bool {
        self.prompt_history.load.is_some()
    }

    pub(crate) fn prompt_history_navigation_pending(&self) -> bool {
        self.prompt_history.navigation.is_some()
    }

    pub(crate) fn cancel_pending_history_navigation(&mut self) {
        self.prompt_history.navigation = None;
    }

    pub(crate) fn request_history_navigation(&mut self, delta: i32) -> bool {
        if !self.prompt_history_can_load() || self.input_history_cursor.is_some() {
            return false;
        }
        if delta > 0 {
            if let Some(navigation) = &mut self.prompt_history.navigation {
                navigation.steps -= 1;
                if navigation.steps == 0 {
                    self.prompt_history.navigation = None;
                }
                return true;
            }
            return false;
        }
        if delta == 0 {
            return false;
        }
        if let Some(navigation) = &mut self.prompt_history.navigation {
            navigation.steps = (navigation.steps + 1).min(MAX_HISTORY_ENTRIES);
            return true;
        }
        match self.prompt_history.request_load() {
            Ok(()) => {
                self.prompt_history.navigation = Some(PendingNavigation {
                    input: self.bottom_pane.input.clone(),
                    cursor: self.bottom_pane.input_cursor_offset,
                    overlay: self.overlay,
                    steps: 1,
                });
                true
            }
            Err(error) => {
                self.push_notice(
                    NoticeLevel::Warning,
                    format!("Could not read prompt history: {error:#}"),
                );
                false
            }
        }
    }

    pub(crate) fn poll_prompt_history(&mut self) -> bool {
        let mut changed = false;
        if let Some(io) = &self.prompt_history.io {
            let status = io.status();
            if status.revision != self.prompt_history.status_revision {
                self.prompt_history.status_revision = status.revision;
                if let Some(error) = status.error {
                    self.push_notice(
                        NoticeLevel::Warning,
                        format!("Prompt history persistence failed: {error}"),
                    );
                    changed = true;
                }
            }
        }
        let result =
            self.prompt_history
                .load
                .as_mut()
                .and_then(|load| match load.receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(oneshot::error::TryRecvError::Empty) => None,
                    Err(oneshot::error::TryRecvError::Closed) => {
                        Some(Err(anyhow::anyhow!("prompt history reader stopped")))
                    }
                });
        let Some(result) = result else {
            return changed;
        };
        let Some(request) = self.prompt_history.load.take() else {
            return changed;
        };
        // Only explicit traversal pins a result across a background refresh.
        // Opening search or editing its query must select the newest match.
        let selected = self.prompt_history.selection_anchor.take();
        match result {
            Ok(read) => {
                self.prompt_history.local.retain(|local| {
                    !local.queued
                        || local.revision > request.revision
                        || read.pending_ids.contains(local.entry.id())
                });
                let mut seen = HashSet::new();
                self.input_history = read
                    .loaded
                    .entries
                    .iter()
                    .chain(self.prompt_history.local.iter().map(|local| &local.entry))
                    .filter(|entry| seen.insert(entry.id().to_owned()))
                    .map(|entry| entry.text().to_owned())
                    .collect();
                if self.input_history.len() > MAX_HISTORY_ENTRIES {
                    self.input_history
                        .drain(..self.input_history.len() - MAX_HISTORY_ENTRIES);
                }
                if self.input_history_cursor.is_some() {
                    self.input_history_cursor = self
                        .input_history
                        .iter()
                        .rposition(|text| *text == self.bottom_pane.input);
                }
                self.prompt_history.selected = selected
                    .as_deref()
                    .and_then(|selected| {
                        self.history_matches()
                            .iter()
                            .position(|text| *text == selected)
                    })
                    .unwrap_or(0);
                if read.loaded.skipped_lines > 0 {
                    self.push_notice(
                        NoticeLevel::Warning,
                        format!(
                            "Prompt history recovered; skipped {} invalid records.",
                            read.loaded.skipped_lines
                        ),
                    );
                }
            }
            Err(error) => {
                self.push_notice(
                    NoticeLevel::Warning,
                    format!("Could not read prompt history: {error:#}"),
                );
            }
        }
        if let Some(navigation) = self.prompt_history.navigation.take()
            && self.bottom_pane.input == navigation.input
            && self.bottom_pane.input_cursor_offset == navigation.cursor
            && self.overlay == navigation.overlay
            && self.active_pending_interaction().is_none()
        {
            for _ in 0..navigation.steps {
                self.navigate_loaded_input_history(-1);
            }
        }
        true
    }

    pub(crate) async fn shutdown_prompt_history(&mut self) -> Result<()> {
        self.prompt_history.load = None;
        if let Some(io) = self.prompt_history.io.take() {
            io.shutdown()
                .await
                .context("flush prompt history on exit")?;
        }
        Ok(())
    }

    pub(crate) fn history_matches(&self) -> Vec<&str> {
        let query = self.prompt_history.query.to_lowercase();
        let mut seen = HashSet::new();
        self.input_history
            .iter()
            .rev()
            .filter(|text| text.to_lowercase().contains(&query) && seen.insert(text.as_str()))
            .map(String::as_str)
            .collect()
    }

    pub(crate) fn apply_history_action(&mut self, action: HistoryAction) {
        match action {
            HistoryAction::Open => {
                self.flush_composer_paste();
                self.prompt_history.navigation = None;
                self.prompt_history.query.clear();
                self.prompt_history.query_cursor = None;
                self.prompt_history.reset_selection();
                self.open_overlay(Overlay::HistorySearch);
                if self.prompt_history_can_load()
                    && let Err(error) = self.prompt_history.request_load()
                {
                    self.push_notice(
                        NoticeLevel::Warning,
                        format!("Could not read prompt history: {error:#}"),
                    );
                }
            }
            HistoryAction::Older => {
                self.prompt_history.selected = (self.prompt_history.selected + 1)
                    .min(self.history_matches().len().saturating_sub(1));
                self.prompt_history.selection_anchor = self
                    .history_matches()
                    .get(self.prompt_history.selected)
                    .map(|text| (*text).to_owned());
            }
            HistoryAction::Newer => {
                self.prompt_history.selected = self.prompt_history.selected.saturating_sub(1);
                self.prompt_history.selection_anchor = self
                    .history_matches()
                    .get(self.prompt_history.selected)
                    .map(|text| (*text).to_owned());
            }
            HistoryAction::Accept => {
                let selected = self
                    .history_matches()
                    .get(self.prompt_history.selected)
                    .map(|text| (*text).to_owned());
                if let Some(text) = selected {
                    self.dismiss_overlay();
                    self.bottom_pane.clear_input();
                    self.bottom_pane.input = text;
                    self.reset_input_history_navigation();
                    self.sync_command_palette_with_input();
                }
            }
        }
    }
}
