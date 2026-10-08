use std::collections::HashSet;
use std::time::{Duration, Instant};

use anyhow::Result;
use rara_state::state_db::{StateDb, ThreadListCursor, ThreadListQuery, ThreadListSort};
use tokio::sync::oneshot;

use super::{NoticeLevel, TuiApp};
use crate::thread_store::ThreadSummary;

const RESUME_PAGE_SIZE: usize = 50;
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, Debug, Default)]
enum ResumeScope {
    #[default]
    Automatic,
    CurrentDirectory,
    All,
}

#[derive(Clone)]
struct ResumeRequest {
    generation: u64,
    session_id: String,
    cwd: String,
    search: String,
    sort: ThreadListSort,
    scope: ResumeScope,
    after: Option<ThreadListCursor>,
    effective_cwd: Option<String>,
}

struct ResumePage {
    threads: Vec<ThreadSummary>,
    next: Option<ThreadListCursor>,
    effective_cwd: Option<String>,
}

struct PendingResumeQuery {
    request: ResumeRequest,
    receiver: oneshot::Receiver<Result<ResumePage>>,
}

#[derive(Default)]
pub(crate) struct ResumeQueryState {
    generation: u64,
    due: Option<Instant>,
    pending: Option<PendingResumeQuery>,
    next: Option<ThreadListCursor>,
    pending_selection: Option<usize>,
    source: Option<(String, String)>,
    scope: ResumeScope,
    effective_cwd: Option<String>,
    has_resolved_scope: bool,
    pub(crate) page_items: usize,
    pub(crate) loading: bool,
    pub(crate) error: Option<String>,
}

impl ResumeQueryState {
    pub(crate) fn has_more(&self) -> bool {
        self.next.is_some()
    }

    pub(crate) fn scope_label(&self) -> &'static str {
        match self.scope {
            ResumeScope::CurrentDirectory => "cwd",
            ResumeScope::All => "all",
            ResumeScope::Automatic
                if !self.has_resolved_scope || (self.loading && self.next.is_none()) =>
            {
                "auto"
            }
            ResumeScope::Automatic if self.effective_cwd.is_some() => "cwd (auto)",
            ResumeScope::Automatic => "all (auto)",
        }
    }
}

impl ResumeRequest {
    fn load(&self, db: &StateDb) -> Result<ResumePage> {
        let cwd = if self.cwd.is_empty() {
            std::env::current_dir()?.to_string_lossy().into_owned()
        } else {
            self.cwd.clone()
        };
        let effective_cwd = if self.after.is_some() {
            self.effective_cwd.clone()
        } else {
            match self.scope {
                ResumeScope::All => None,
                ResumeScope::CurrentDirectory => Some(cwd),
                ResumeScope::Automatic => {
                    let page = db.query_threads(ThreadListQuery {
                        search: "",
                        cwd: Some(&cwd),
                        exclude_session_id: Some(&self.session_id),
                        sort: self.sort,
                        after: None,
                        limit: 1,
                    })?;
                    (!page.threads.is_empty()).then_some(cwd)
                }
            }
        };
        let page = db.query_threads(ThreadListQuery {
            search: &self.search,
            cwd: effective_cwd.as_deref(),
            exclude_session_id: Some(&self.session_id),
            sort: self.sort,
            after: self.after.as_ref(),
            limit: RESUME_PAGE_SIZE,
        })?;
        Ok(ResumePage {
            threads: page.threads.into_iter().map(ThreadSummary::from).collect(),
            next: page.next_cursor,
            effective_cwd,
        })
    }
}

impl TuiApp {
    pub(super) fn refresh_recent_threads(&mut self) {
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(crate) fn refresh_recent_threads_for_resume_picker(&mut self) {
        self.resume_query.source =
            Some((self.snapshot.session_id.clone(), self.snapshot.cwd.clone()));
        self.resume_query.generation = self.resume_query.generation.wrapping_add(1);
        self.resume_query.pending = None;
        self.resume_query.next = None;
        self.resume_query.pending_selection = None;
        self.resume_query.error = None;
        self.resume_query.loading = self.storage.is_some() && self.state_db.is_some();
        self.resume_query.due = self.resume_query.loading.then(Instant::now);
        self.recent_threads.clear();
        self.resume_picker_idx = 0;
    }

    pub(crate) fn cycle_resume_sort(&mut self) {
        self.resume_sort_by_created = !self.resume_sort_by_created;
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(crate) fn toggle_resume_scope(&mut self) {
        self.resume_query.scope = match self.resume_query.scope {
            ResumeScope::CurrentDirectory => ResumeScope::All,
            ResumeScope::All => ResumeScope::CurrentDirectory,
            ResumeScope::Automatic
                if self.resume_query.effective_cwd.is_some()
                    || !self.resume_query.has_resolved_scope =>
            {
                ResumeScope::All
            }
            ResumeScope::Automatic => ResumeScope::CurrentDirectory,
        };
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(super) fn resume_search_changed(&mut self) {
        self.refresh_recent_threads_for_resume_picker();
        self.resume_query.due = self
            .resume_query
            .loading
            .then(|| Instant::now() + SEARCH_DEBOUNCE);
    }

    pub(crate) fn clear_resume_search(&mut self) {
        self.resume_search_query.clear();
        self.resume_search_cursor_offset = None;
        self.refresh_recent_threads_for_resume_picker();
    }

    pub(crate) fn move_resume_selection(&mut self, delta: i32) {
        if self.recent_threads.is_empty() {
            return;
        }
        let target = self.resume_picker_idx.saturating_add_signed(delta as isize);
        self.resume_picker_idx = target.min(self.recent_threads.len() - 1);
        if delta < 0 {
            self.resume_query.pending_selection = None;
        }
        if target >= self.recent_threads.len()
            && self.resume_query.next.is_some()
            && !self.resume_query.loading
        {
            self.resume_query.pending_selection = Some(target);
            self.resume_query.loading = true;
            self.resume_query.error = None;
            self.resume_query.due = Some(Instant::now());
        }
    }

    /// Starts the debounced query once it is due. Returns true when the
    /// request failed to start and its error was already applied.
    ///
    /// Kept separate from the result poll so a fast worker cannot complete the
    /// request inside the same call that dispatches it.
    pub(crate) fn dispatch_due_resume_query(&mut self) -> bool {
        if !self
            .resume_query
            .due
            .is_some_and(|due| due <= Instant::now())
        {
            return false;
        }
        self.resume_query.due = None;
        let Some(db) = self.state_db.clone() else {
            return false;
        };
        let Some(storage) = &self.storage else {
            return false;
        };
        let request = ResumeRequest {
            generation: self.resume_query.generation,
            session_id: self.snapshot.session_id.clone(),
            cwd: self.snapshot.cwd.clone(),
            search: self.resume_search_query.clone(),
            sort: if self.resume_sort_by_created {
                ThreadListSort::Created
            } else {
                ThreadListSort::Updated
            },
            scope: self.resume_query.scope,
            after: self.resume_query.next.clone(),
            effective_cwd: self.resume_query.effective_cwd.clone(),
        };
        let work = request.clone();
        match storage.read(move || work.load(&db)) {
            Ok(receiver) => {
                self.resume_query.pending = Some(PendingResumeQuery { request, receiver });
                false
            }
            Err(error) => {
                self.finish_resume_query(request, Err(error));
                true
            }
        }
    }

    pub(crate) fn poll_resume_queries(&mut self) -> bool {
        let source_changed = self
            .resume_query
            .source
            .as_ref()
            .is_some_and(|(session_id, cwd)| {
                session_id != &self.snapshot.session_id || cwd != &self.snapshot.cwd
            });
        if source_changed {
            self.refresh_recent_threads_for_resume_picker();
        }
        if self.dispatch_due_resume_query() {
            return true;
        }
        let Some(pending) = &mut self.resume_query.pending else {
            return source_changed;
        };
        let result = match pending.receiver.try_recv() {
            Ok(result) => result,
            Err(oneshot::error::TryRecvError::Empty) => return source_changed,
            Err(oneshot::error::TryRecvError::Closed) => Err(anyhow::anyhow!(
                "storage worker stopped before listing threads"
            )),
        };
        let Some(pending) = self.resume_query.pending.take() else {
            return false;
        };
        self.finish_resume_query(pending.request, result);
        true
    }

    fn finish_resume_query(&mut self, request: ResumeRequest, result: Result<ResumePage>) {
        if request.generation != self.resume_query.generation {
            return;
        }
        if request.session_id != self.snapshot.session_id || request.cwd != self.snapshot.cwd {
            self.refresh_recent_threads_for_resume_picker();
            return;
        }
        self.resume_query.loading = false;
        match result {
            Ok(page) => {
                if request.after.is_none() {
                    self.recent_threads.clear();
                }
                let mut known = self
                    .recent_threads
                    .iter()
                    .map(|thread| thread.metadata.session_id.clone())
                    .collect::<HashSet<_>>();
                self.recent_threads.extend(
                    page.threads
                        .into_iter()
                        .filter(|thread| known.insert(thread.metadata.session_id.clone())),
                );
                self.resume_query.next = page.next;
                self.resume_query.effective_cwd = page.effective_cwd;
                self.resume_query.has_resolved_scope = true;
                self.resume_picker_idx = self
                    .resume_query
                    .pending_selection
                    .take()
                    .unwrap_or(self.resume_picker_idx)
                    .min(self.recent_threads.len().saturating_sub(1));
            }
            Err(error) => {
                let message = format!("Could not list saved threads: {error:#}");
                log::warn!("{message}");
                self.resume_query.error = Some(message.clone());
                self.resume_query.pending_selection = None;
                self.push_notice(NoticeLevel::Warning, message);
            }
        }
    }

    #[cfg(test)]
    pub(crate) async fn finish_resume_query_for_test(&mut self) {
        if self.resume_query.due.is_some() {
            self.resume_query.due = Some(Instant::now());
        }
        self.poll_resume_queries();
        if let Some(pending) = self.resume_query.pending.take() {
            let result = tokio::time::timeout(Duration::from_secs(5), pending.receiver)
                .await
                .unwrap()
                .unwrap();
            self.finish_resume_query(pending.request, result);
        }
    }
}

#[cfg(test)]
#[path = "resume_query_tests.rs"]
mod tests;
