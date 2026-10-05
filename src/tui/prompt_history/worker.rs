use std::collections::{HashSet, VecDeque};
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;

use anyhow::{Context, Result};
use rara_persistence::prompt_history::{
    MAX_HISTORY_ENTRIES, PromptHistoryEntry, PromptHistoryLoad, PromptHistoryStore,
};
use tokio::sync::oneshot;

enum Request {
    Append(PromptHistoryEntry),
    Load(oneshot::Sender<Result<HistoryRead>>),
}

pub(super) struct HistoryRead {
    pub loaded: PromptHistoryLoad,
    pub pending_ids: HashSet<String>,
}

#[derive(Clone, Default)]
pub(super) struct Status {
    pub revision: u64,
    pub error: Option<String>,
}

pub(super) struct HistoryIo {
    sender: mpsc::SyncSender<Request>,
    worker: JoinHandle<Result<()>>,
    status: Arc<Mutex<Status>>,
}

impl HistoryIo {
    pub fn start(store: PromptHistoryStore) -> Result<Self> {
        let (sender, receiver) = mpsc::sync_channel(MAX_HISTORY_ENTRIES);
        let status = Arc::new(Mutex::new(Status::default()));
        let worker_status = Arc::clone(&status);
        let worker = std::thread::Builder::new()
            .name("prompt-history".into())
            .spawn(move || run(store, receiver, worker_status))
            .context("start prompt history worker")?;
        Ok(Self {
            sender,
            worker,
            status,
        })
    }

    pub fn append(&self, entry: PromptHistoryEntry) -> Result<()> {
        self.send(Request::Append(entry))
    }

    pub fn load(&self) -> Result<oneshot::Receiver<Result<HistoryRead>>> {
        let (sender, receiver) = oneshot::channel();
        self.send(Request::Load(sender))?;
        Ok(receiver)
    }

    fn send(&self, request: Request) -> Result<()> {
        self.sender.try_send(request).map_err(|error| match error {
            mpsc::TrySendError::Full(_) => anyhow::anyhow!("prompt history queue is full"),
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!("prompt history worker stopped"),
        })
    }

    pub fn status(&self) -> Status {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub async fn shutdown(self) -> Result<()> {
        drop(self.sender);
        tokio::task::spawn_blocking(move || self.worker.join())
            .await
            .context("join prompt history cleanup")?
            .map_err(|_| anyhow::anyhow!("prompt history worker panicked"))?
    }
}

fn publish(status: &Mutex<Status>, error: Option<String>) {
    let mut status = status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if status.error != error {
        status.revision = status.revision.wrapping_add(1);
        if let Some(error) = &error {
            log::warn!("Prompt history persistence failed: {error}");
        }
        status.error = error;
    }
}

fn flush(store: &PromptHistoryStore, pending: &mut VecDeque<PromptHistoryEntry>) -> Result<()> {
    while let Some(entry) = pending.front() {
        store.append(entry)?;
        pending.pop_front();
    }
    Ok(())
}

fn run(
    store: PromptHistoryStore,
    receiver: mpsc::Receiver<Request>,
    status: Arc<Mutex<Status>>,
) -> Result<()> {
    let mut pending = VecDeque::new();
    let mut dropped = false;
    for request in receiver {
        match request {
            Request::Append(entry) => {
                if pending.len() == MAX_HISTORY_ENTRIES {
                    let error = flush(&store, &mut pending)
                        .err()
                        .map(|error| format!("{error:#}"));
                    publish(&status, error);
                }
                if pending.len() == MAX_HISTORY_ENTRIES {
                    dropped = true;
                    publish(
                        &status,
                        Some("history write backlog is full; a prompt was not saved".into()),
                    );
                    continue;
                }
                pending.push_back(entry);
                let error = flush(&store, &mut pending)
                    .err()
                    .map(|error| format!("{error:#}"));
                publish(&status, error);
            }
            Request::Load(reply) => {
                let error = flush(&store, &mut pending)
                    .err()
                    .map(|error| format!("{error:#}"));
                publish(&status, error);
                let result = store.load().map(|loaded| HistoryRead {
                    loaded,
                    pending_ids: pending.iter().map(|entry| entry.id().to_owned()).collect(),
                });
                // Search/navigation can be dismissed while the disk read is running.
                if let Err(Err(error)) = reply.send(result) {
                    log::warn!("Discarded prompt history read failed: {error:#}");
                }
            }
        }
    }
    flush(&store, &mut pending).inspect_err(|error| {
        log::warn!("Prompt history shutdown failed: {error:#}");
    })?;
    anyhow::ensure!(
        !dropped,
        "some prompts could not be saved because the history backlog was full"
    );
    Ok(())
}
