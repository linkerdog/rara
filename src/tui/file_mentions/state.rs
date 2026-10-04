use std::ops::Range;
use std::path::PathBuf;

use rara_file_search::{FileMatch, IndexedSearchResults};
use tokio::time::{Duration, Instant};

use super::worker::{FileSearchWorker, SearchRequest, SearchResponse};
use crate::tui::composer_atoms::encode_mention;
use crate::tui::state::{TuiApp, char_offset_to_byte_index};

const DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug)]
pub(crate) enum FileMentionAction {
    Previous,
    Next,
    Accept,
    Dismiss,
}

#[derive(Clone, PartialEq, Eq)]
struct QueryOwner {
    input: String,
    cursor: usize,
    session: String,
    root: PathBuf,
}

impl QueryOwner {
    fn matches(&self, app: &TuiApp) -> bool {
        self.input == app.bottom_pane.input
            && self.cursor == app.composer_cursor_offset()
            && self.session == app.snapshot.session_id
            && self.root == std::path::Path::new(&app.snapshot.cwd)
    }
}

struct ActiveQuery {
    owner: QueryOwner,
    range: Range<usize>,
    text: String,
    generation: u64,
    due: Option<Instant>,
}

#[derive(Default)]
pub(super) enum SearchStatus {
    #[default]
    Searching,
    Ready {
        truncated: bool,
        skipped_non_utf8: bool,
    },
    Failed,
}

#[derive(Default)]
pub(crate) struct FileMentionState {
    query: Option<ActiveQuery>,
    dismissed: Option<QueryOwner>,
    generation: u64,
    worker: Option<FileSearchWorker>,
    pub(super) matches: Vec<FileMatch>,
    pub(super) selected: usize,
    pub(super) status: SearchStatus,
}

impl TuiApp {
    fn file_mentions_own_input(&self) -> bool {
        self.overlay.is_none()
            && self.active_pending_interaction().is_none()
            && self.composer_input_is_active()
    }

    pub(crate) fn file_mention_open(&self) -> bool {
        self.file_mentions_own_input()
            && self
                .file_mentions
                .query
                .as_ref()
                .is_some_and(|query| query.owner.matches(self))
    }

    pub(crate) fn refresh_file_mentions(&mut self) -> bool {
        if self
            .file_mentions
            .dismissed
            .as_ref()
            .is_some_and(|owner| !owner.matches(self))
        {
            self.file_mentions.dismissed = None;
        }
        if self.file_mentions_own_input()
            && self
                .file_mentions
                .query
                .as_ref()
                .is_some_and(|query| query.owner.matches(self))
        {
            return false;
        }
        let query = self
            .file_mentions_own_input()
            .then(|| mention_query(self))
            .flatten();
        let Some((range, text)) = query else {
            return self.close_file_mentions();
        };
        let owner = QueryOwner {
            input: self.bottom_pane.input.clone(),
            cursor: self.composer_cursor_offset(),
            session: self.snapshot.session_id.clone(),
            root: PathBuf::from(&self.snapshot.cwd),
        };
        if self.file_mentions.dismissed.as_ref() == Some(&owner) {
            return self.close_file_mentions();
        }
        if self.file_mentions.query.as_ref().is_some_and(|query| {
            query.owner.root != owner.root || query.owner.session != owner.session
        }) && let Some(worker) = &self.file_mentions.worker
        {
            worker.clear();
        }
        self.transcript_selection.clear();
        self.file_mentions.generation = self.file_mentions.generation.wrapping_add(1);
        self.file_mentions.query = Some(ActiveQuery {
            owner,
            range,
            text,
            generation: self.file_mentions.generation,
            due: Some(Instant::now() + DEBOUNCE),
        });
        self.file_mentions.matches.clear();
        self.file_mentions.selected = 0;
        self.file_mentions.status = SearchStatus::Searching;
        true
    }

    fn close_file_mentions(&mut self) -> bool {
        let was_open = self.file_mentions.query.take().is_some();
        if was_open {
            if let Some(worker) = &self.file_mentions.worker {
                worker.clear();
            }
            self.file_mentions.matches.clear();
        }
        was_open
    }

    pub(crate) fn poll_file_mentions(&mut self) -> bool {
        let mut changed = self.refresh_file_mentions();
        if self
            .file_mentions
            .query
            .as_ref()
            .is_some_and(|query| query.due.is_some_and(|due| Instant::now() >= due))
        {
            let request = self.file_mentions.query.as_mut().map(|query| {
                query.due = None;
                SearchRequest {
                    generation: query.generation,
                    root: query.owner.root.clone(),
                    query: query.text.clone(),
                }
            });
            if let Some(request) = request {
                let result = (|| {
                    if self.file_mentions.worker.is_none() {
                        self.file_mentions.worker = Some(FileSearchWorker::start()?);
                    }
                    if let Some(worker) = &self.file_mentions.worker {
                        worker.request(request);
                    }
                    anyhow::Ok(())
                })();
                if let Err(error) = result {
                    self.file_search_failed(error);
                }
                changed = true;
            }
        }
        if let Some(worker) = &mut self.file_mentions.worker {
            match worker.poll() {
                Ok(Some(response)) => changed |= self.apply_file_response(response),
                Ok(None) => {}
                Err(error) => {
                    self.file_mentions.worker = None;
                    self.file_search_failed(error);
                    changed = true;
                }
            }
        }
        changed
    }

    // Validate both generation and current ownership at the publication boundary.
    fn apply_file_response(&mut self, response: SearchResponse) -> bool {
        if !self.file_mention_open()
            || !self
                .file_mentions
                .query
                .as_ref()
                .is_some_and(|query| query.generation == response.generation)
        {
            return false;
        }
        match response.result {
            Ok(result) => self.apply_file_matches(result),
            Err(error) => self.file_search_failed(error),
        }
        true
    }

    fn apply_file_matches(&mut self, result: IndexedSearchResults) {
        self.file_mentions.status = SearchStatus::Ready {
            truncated: result.index_truncated,
            skipped_non_utf8: result.skipped_non_utf8 > 0,
        };
        self.file_mentions.matches = result.results.matches;
        self.file_mentions.selected = 0;
    }

    fn file_search_failed(&mut self, error: anyhow::Error) {
        log::warn!("File mention search failed: {error:#}");
        self.file_mentions.matches.clear();
        self.file_mentions.status = SearchStatus::Failed;
        self.push_notice(format!("File search failed: {error:#}"));
    }

    pub(crate) fn apply_file_mention(&mut self, action: FileMentionAction) {
        self.refresh_file_mentions();
        if !self.file_mention_open() {
            return;
        }
        match action {
            FileMentionAction::Previous => {
                self.file_mentions.selected = self.file_mentions.selected.saturating_sub(1)
            }
            FileMentionAction::Next => {
                self.file_mentions.selected = (self.file_mentions.selected + 1)
                    .min(self.file_mentions.matches.len().saturating_sub(1))
            }
            FileMentionAction::Dismiss => {
                self.file_mentions.dismissed = self
                    .file_mentions
                    .query
                    .as_ref()
                    .map(|query| query.owner.clone());
                self.close_file_mentions();
            }
            FileMentionAction::Accept => {
                let selection = self
                    .file_mentions
                    .matches
                    .get(self.file_mentions.selected)
                    .and_then(|entry| entry.path.to_str())
                    .zip(self.file_mentions.query.as_ref())
                    .map(|(path, query)| (encode_mention(path), query.range.clone()));
                if let Some((token, range)) = selection {
                    self.bottom_pane.edit_composer(range, &format!("{token} "));
                    self.reset_input_history_navigation();
                    self.close_file_mentions();
                    self.sync_command_palette_with_input();
                }
            }
        }
    }
}

fn mention_query(app: &TuiApp) -> Option<(Range<usize>, String)> {
    let cursor = app.composer_cursor_offset();
    if app
        .bottom_pane
        .composer_atoms()
        .iter()
        .any(|atom| atom.range.start <= cursor && cursor <= atom.range.end)
    {
        return None;
    }
    let byte = char_offset_to_byte_index(&app.bottom_pane.input, cursor);
    let prefix = &app.bottom_pane.input[..byte];
    let start = prefix.rfind(char::is_whitespace).map_or(0, |offset| {
        offset + prefix[offset..].chars().next().map_or(0, char::len_utf8)
    });
    let token = &prefix[start..];
    let query = token.strip_prefix('@')?;
    if query.starts_with('"') || query.contains('@') {
        return None;
    }
    let start = prefix[..start].chars().count();
    let suffix = app.bottom_pane.input[byte..]
        .chars()
        .take_while(|ch| !ch.is_whitespace())
        .count();
    Some((start..cursor + suffix, query.to_owned()))
}

#[cfg(test)]
#[path = "tests.rs"]
mod tests;
