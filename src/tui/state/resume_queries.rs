use std::time::{Duration, Instant};

use anyhow::Result;
use tokio::sync::oneshot;

use super::TuiApp;
use crate::thread_store::{ThreadStore, ThreadSummary};

const RESUME_PICKER_THREAD_LIMIT: usize = 200;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Default)]
pub(crate) struct ResumeQueryState {
    generation: u64,
    due: Option<Instant>,
    pending: Option<(u64, oneshot::Receiver<Result<Vec<ThreadSummary>>>)>,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

impl TuiApp {
    pub(super) fn refresh_recent_threads(&mut self) {
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(super) fn refresh_recent_threads_for_resume_picker(&mut self) {
        self.resume_query.generation = self.resume_query.generation.wrapping_add(1);
        self.resume_query.pending = None;
        self.resume_query.error = None;
        self.resume_query.loading = self.storage.is_some() && self.state_db.is_some();
        self.resume_query.due = self.resume_query.loading.then(Instant::now);
        if !self.resume_query.loading {
            self.recent_threads.clear();
        }
    }

    pub(crate) fn cycle_resume_sort(&mut self) {
        self.resume_sort_by_created = !self.resume_sort_by_created;
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(crate) fn push_resume_search_char(&mut self, c: char) {
        self.insert_resume_search_text(&c.to_string());
    }

    pub(crate) fn insert_resume_search_text(&mut self, text: &str) {
        self.resume_search_query.push_str(text);
        self.resume_picker_idx = 0;
        self.refresh_recent_threads_for_resume_picker();
        self.resume_query.due = self
            .resume_query
            .loading
            .then(|| Instant::now() + SEARCH_DEBOUNCE);
    }

    pub(crate) fn pop_resume_search_char(&mut self) {
        self.resume_search_query.pop();
        self.resume_picker_idx = 0;
        self.refresh_recent_threads_for_resume_picker();
        self.resume_query.due = self
            .resume_query
            .loading
            .then(|| Instant::now() + SEARCH_DEBOUNCE);
    }

    pub(crate) fn clear_resume_search(&mut self) {
        self.resume_search_query.clear();
        self.resume_picker_idx = 0;
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(crate) fn poll_resume_queries(&mut self) -> bool {
        if self
            .resume_query
            .due
            .is_some_and(|due| due <= Instant::now())
        {
            self.resume_query.due = None;
            let Some(db) = self.state_db.clone() else {
                return false;
            };
            let Some(storage) = &self.storage else {
                return false;
            };
            let generation = self.resume_query.generation;
            let cwd = self.snapshot.cwd.clone();
            let query = self.resume_search_query.trim().to_ascii_lowercase();
            let by_created = self.resume_sort_by_created;
            match storage.read(move || {
                let cwd = if cwd.is_empty() {
                    std::env::current_dir()?.to_string_lossy().into_owned()
                } else {
                    cwd
                };
                let mut threads =
                    ThreadStore::list_recent_threads_for_db(&db, RESUME_PICKER_THREAD_LIMIT)?;
                if threads.iter().any(|thread| thread.metadata.cwd == cwd) {
                    threads.retain(|thread| thread.metadata.cwd == cwd);
                }
                if !query.is_empty() {
                    threads.retain(|thread| matches_query(thread, &query));
                }
                threads.sort_by(|a, b| {
                    if by_created {
                        b.metadata.created_at.cmp(&a.metadata.created_at)
                    } else {
                        b.metadata.updated_at.cmp(&a.metadata.updated_at)
                    }
                });
                Ok(threads)
            }) {
                Ok(receiver) => self.resume_query.pending = Some((generation, receiver)),
                Err(error) => {
                    self.finish_resume_query(generation, Err(error));
                    return true;
                }
            }
        }
        let Some((generation, receiver)) = &mut self.resume_query.pending else {
            return false;
        };
        let result = match receiver.try_recv() {
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Empty) => return false,
            Err(oneshot::error::TryRecvError::Closed) => Err(anyhow::anyhow!(
                "storage worker stopped before listing threads"
            )),
        };
        let generation = *generation;
        self.resume_query.pending = None;
        self.finish_resume_query(generation, result);
        true
    }

    fn finish_resume_query(&mut self, generation: u64, result: Result<Vec<ThreadSummary>>) {
        if generation != self.resume_query.generation {
            return;
        }
        self.resume_query.loading = false;
        match result {
            Ok(threads) => {
                self.recent_threads = threads;
                self.resume_picker_idx = self
                    .resume_picker_idx
                    .min(self.recent_threads.len().saturating_sub(1));
            }
            Err(error) => {
                log::warn!("Could not list saved threads: {error:#}");
                self.resume_query.error = Some(format!("Could not list saved threads: {error:#}"));
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn finish_resume_query_for_test(&mut self) {
        if self.resume_query.due.is_some() {
            self.resume_query.due = Some(Instant::now());
        }
        self.poll_resume_queries();
        if let Some((generation, receiver)) = self.resume_query.pending.take() {
            let result = tokio::time::timeout(Duration::from_secs(5), receiver)
                .await
                .unwrap()
                .unwrap();
            self.finish_resume_query(generation, result);
        }
    }
}

fn matches_query(thread: &ThreadSummary, query: &str) -> bool {
    let metadata = &thread.metadata;
    [
        thread.preview.as_str(),
        &metadata.session_id,
        &metadata.cwd,
        &metadata.branch,
        &metadata.provider,
        &metadata.model,
        &metadata.agent_mode,
        &metadata.bash_approval,
    ]
    .into_iter()
    .any(|value| value.to_ascii_lowercase().contains(query))
}
