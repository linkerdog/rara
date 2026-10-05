use std::num::NonZeroUsize;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use rara_file_search::{
    FileSearchIndex, FileSearchOptions, IndexLimits, IndexedSearchResults, SearchCancellation,
};

#[derive(Clone)]
pub(super) struct SearchRequest {
    pub generation: u64,
    pub root: PathBuf,
    pub query: String,
}

pub(super) struct SearchResponse {
    pub generation: u64,
    pub result: Result<IndexedSearchResults>,
}

#[derive(Default)]
struct Pending {
    request: Option<SearchRequest>,
    response: Option<SearchResponse>,
    changed: bool,
    stopped: bool,
    index_epoch: u64,
    discovery: SearchCancellation,
    ranking: SearchCancellation,
}

#[derive(Default)]
struct Shared {
    pending: Mutex<Pending>,
    changed: Condvar,
    #[cfg(test)]
    completed: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Pending> {
        self.pending.lock().unwrap_or_else(|poisoned| {
            log::warn!("File search worker state was poisoned; recovering");
            poisoned.into_inner()
        })
    }
}

/// One worker and replaceable request/result slots per TUI, including while closed.
pub(super) struct FileSearchWorker {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl FileSearchWorker {
    pub fn start() -> Result<Self> {
        let shared = Arc::new(Shared::default());
        let worker = shared.clone();
        let thread = std::thread::Builder::new()
            .name("tui-file-search".into())
            .spawn(move || run(worker))
            .context("start file search worker")?;
        Ok(Self {
            shared,
            thread: Some(thread),
        })
    }

    pub fn request(&self, request: SearchRequest) {
        let mut pending = self.shared.lock();
        if pending
            .request
            .as_ref()
            .is_none_or(|old| old.root != request.root)
        {
            pending.discovery.cancel();
            pending.discovery = SearchCancellation::default();
            pending.index_epoch = pending.index_epoch.wrapping_add(1);
        }
        pending.ranking.cancel();
        pending.ranking = SearchCancellation::default();
        pending.request = Some(request);
        pending.response = None;
        pending.changed = true;
        self.shared.changed.notify_one();
    }

    pub fn clear(&self) {
        let mut pending = self.shared.lock();
        pending.discovery.cancel();
        pending.index_epoch = pending.index_epoch.wrapping_add(1);
        pending.ranking.cancel();
        pending.request = None;
        pending.response = None;
        pending.changed = true;
        self.shared.changed.notify_one();
    }

    pub fn poll(&mut self) -> Result<Option<SearchResponse>> {
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(thread) = self.thread.take() {
                thread
                    .join()
                    .map_err(|_| anyhow::anyhow!("file search worker panicked"))?;
            }
            anyhow::bail!("file search worker stopped unexpectedly");
        }
        Ok(self.shared.lock().response.take())
    }
}

impl Drop for FileSearchWorker {
    fn drop(&mut self) {
        let mut pending = self.shared.lock();
        pending.stopped = true;
        pending.discovery.cancel();
        pending.ranking.cancel();
        self.shared.changed.notify_one();
        drop(pending);
        // Never join filesystem work on the terminal task. A finished thread
        // can be reaped without waiting; a live scan observes cancellation.
        if self.thread.as_ref().is_some_and(JoinHandle::is_finished)
            && let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::warn!("File search worker panicked during shutdown");
        }
    }
}

fn run(shared: Arc<Shared>) {
    let mut index: Option<(u64, FileSearchIndex)> = None;
    loop {
        let (request, epoch, discovery, ranking) = {
            let mut pending = shared.lock();
            while !pending.changed && !pending.stopped {
                pending = shared.changed.wait(pending).unwrap_or_else(|poisoned| {
                    log::warn!("File search worker wait was poisoned; recovering");
                    poisoned.into_inner()
                });
            }
            if pending.stopped {
                return;
            }
            pending.changed = false;
            (
                pending.request.clone(),
                pending.index_epoch,
                pending.discovery.clone(),
                pending.ranking.clone(),
            )
        };
        let Some(request) = request else {
            index = None;
            continue;
        };
        let result = search(&mut index, epoch, &request, &discovery, &ranking);
        let mut pending = shared.lock();
        if !pending.stopped
            && pending
                .request
                .as_ref()
                .is_some_and(|latest| latest.generation == request.generation)
        {
            match result {
                Ok(Some(result)) => {
                    pending.response = Some(SearchResponse {
                        generation: request.generation,
                        result: Ok(result),
                    })
                }
                Err(error) => {
                    pending.response = Some(SearchResponse {
                        generation: request.generation,
                        result: Err(error),
                    })
                }
                Ok(None) => {}
            }
            #[cfg(test)]
            shared.completed.notify_all();
        }
    }
}

fn search(
    index: &mut Option<(u64, FileSearchIndex)>,
    epoch: u64,
    request: &SearchRequest,
    discovery: &SearchCancellation,
    ranking: &SearchCancellation,
) -> Result<Option<IndexedSearchResults>> {
    if index.as_ref().is_none_or(|(current, _)| *current != epoch) {
        *index = None;
        let options = FileSearchOptions {
            follow_links: false,
            exclude: [
                ".git",
                "target",
                "node_modules",
                "dist",
                "build",
                ".venv",
                "venv",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            ..Default::default()
        };
        let limits = IndexLimits {
            max_files: NonZeroUsize::new(100_000).unwrap_or(NonZeroUsize::MIN),
            max_path_bytes: NonZeroUsize::new(32 * 1024 * 1024).unwrap_or(NonZeroUsize::MIN),
        };
        let Some(built) =
            FileSearchIndex::build(request.root.clone(), &options, limits, discovery)?
        else {
            return Ok(None);
        };
        *index = Some((epoch, built));
    }
    Ok(index.as_ref().and_then(|(_, index)| {
        index.search(
            &request.query,
            NonZeroUsize::new(50).unwrap_or(NonZeroUsize::MIN),
            ranking,
        )
    }))
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
