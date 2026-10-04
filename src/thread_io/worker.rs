use std::collections::VecDeque;
use std::sync::{Arc, Mutex, mpsc};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use rara_state::state_db::StateDb;
use tokio::sync::oneshot;

use super::WriteOperation;

const WRITE_INTERVAL: Duration = Duration::from_millis(100);
const RETRY_INTERVAL: Duration = Duration::from_secs(1);

enum FlushMode {
    Timed,
    #[cfg(test)]
    Barriers,
}

/// Applies one ordered persistence operation. Implementations must support
/// retrying an operation after partial I/O without removing its recovery data.
pub(super) trait WriteStore: Send + 'static {
    fn write(&self, operation: &WriteOperation) -> Result<()>;
}

struct DatabaseStore(Arc<StateDb>);

impl WriteStore for DatabaseStore {
    fn write(&self, operation: &WriteOperation) -> Result<()> {
        operation.execute(&self.0)
    }
}

enum Request {
    Write(WriteOperation),
    Read(Box<dyn FnOnce(Result<()>) + Send>),
    Shutdown(oneshot::Sender<Result<()>>),
}

#[derive(Clone, Default, PartialEq, Eq)]
pub(crate) struct StorageStatus {
    pub revision: u64,
    pub error: Option<String>,
}

pub(crate) struct ThreadIo {
    sender: mpsc::Sender<Request>,
    worker: Option<JoinHandle<()>>,
    status: Arc<Mutex<StorageStatus>>,
}

impl ThreadIo {
    pub(crate) fn new(db: Arc<StateDb>) -> Result<Self> {
        Self::with_store(Box::new(DatabaseStore(db)))
    }

    pub(super) fn with_store(store: Box<dyn WriteStore>) -> Result<Self> {
        Self::start(store, FlushMode::Timed)
    }

    #[cfg(test)]
    pub(super) fn with_store_on_barriers(store: Box<dyn WriteStore>) -> Result<Self> {
        Self::start(store, FlushMode::Barriers)
    }

    fn start(store: Box<dyn WriteStore>, mode: FlushMode) -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        let status = Arc::new(Mutex::new(StorageStatus::default()));
        let worker_status = status.clone();
        let worker = std::thread::Builder::new()
            .name("tui-storage".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run(store, receiver, &worker_status, mode);
                }));
                if result.is_err() {
                    set_status(&worker_status, Some("storage worker panicked".into()));
                    log::warn!("Storage worker panicked; pending writes were not acknowledged");
                }
            })
            .context("start storage worker")?;
        Ok(Self {
            sender,
            worker: Some(worker),
            status,
        })
    }

    pub(crate) fn submit(&self, operation: WriteOperation) -> Result<()> {
        self.sender
            .send(Request::Write(operation))
            .map_err(|_| anyhow::anyhow!("storage worker is unavailable"))
    }

    /// Runs a read only after preceding writes succeed. The returned receiver
    /// never owns the worker, so cancelling a read cannot cancel durable writes.
    pub(crate) fn read<T: Send + 'static>(
        &self,
        read: impl FnOnce() -> Result<T> + Send + 'static,
    ) -> Result<oneshot::Receiver<Result<T>>> {
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Request::Read(Box::new(move |flushed| {
                if sender.is_closed() {
                    return;
                }
                let result = flushed.and_then(|()| read());
                // A stale picker/restore request may intentionally drop its receiver.
                if let Err(result) = sender.send(result)
                    && let Err(error) = result
                {
                    log::warn!("Discarded storage request failed: {error:#}");
                }
            })))
            .map_err(|_| anyhow::anyhow!("storage worker is unavailable"))?;
        Ok(receiver)
    }

    pub(crate) async fn flush(&self) -> Result<()> {
        self.read(|| Ok(()))?
            .await
            .context("storage worker stopped before flush acknowledgement")?
    }

    pub(crate) fn status(&self) -> StorageStatus {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone()
    }

    pub(crate) async fn shutdown(&mut self) -> Result<()> {
        if self.worker.is_none() {
            return Ok(());
        }
        let (sender, receiver) = oneshot::channel();
        self.sender
            .send(Request::Shutdown(sender))
            .map_err(|_| anyhow::anyhow!("storage worker is unavailable"))?;
        receiver
            .await
            .context("storage worker stopped before shutdown acknowledgement")??;
        if let Some(worker) = self.worker.take() {
            tokio::task::spawn_blocking(move || worker.join())
                .await
                .context("join storage cleanup task")?
                .map_err(|_| anyhow::anyhow!("storage worker panicked during shutdown"))?;
        }
        Ok(())
    }
}

fn set_status(status: &Mutex<StorageStatus>, error: Option<String>) {
    let mut status = status
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    status.revision = status.revision.wrapping_add(1);
    status.error = error;
}

fn flush_pending(
    store: &dyn WriteStore,
    pending: &mut VecDeque<WriteOperation>,
    status: &Mutex<StorageStatus>,
) -> Result<()> {
    while let Some(operation) = pending.front() {
        if let Err(error) = store.write(operation) {
            let message = format!("{error:#}");
            let previous = status
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .error
                .clone();
            if previous.as_deref() != Some(&message) {
                log::warn!("Background storage failed; retaining pending writes: {message}");
                set_status(status, Some(message));
            }
            return Err(error);
        }
        pending.pop_front();
        set_status(status, None);
    }
    Ok(())
}

fn run(
    store: Box<dyn WriteStore>,
    receiver: mpsc::Receiver<Request>,
    status: &Mutex<StorageStatus>,
    mode: FlushMode,
) {
    let mut pending = VecDeque::<WriteOperation>::new();
    let mut can_merge = false;
    let mut deadline = Instant::now() + WRITE_INTERVAL;
    loop {
        let request = match mode {
            FlushMode::Timed => {
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            }
            #[cfg(test)]
            FlushMode::Barriers => receiver
                .recv()
                .map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        match request {
            Ok(Request::Write(operation)) => {
                let unmerged = match pending.back_mut().filter(|_| can_merge) {
                    Some(previous) => previous.merge(operation).err(),
                    None => Some(operation),
                };
                if let Some(operation) = unmerged {
                    pending.push_back(operation);
                }
                can_merge = true;
            }
            Ok(Request::Read(read)) => {
                can_merge = false;
                read(flush_pending(&*store, &mut pending, status));
                deadline = Instant::now() + WRITE_INTERVAL;
            }
            Ok(Request::Shutdown(reply)) => {
                can_merge = false;
                let result = flush_pending(&*store, &mut pending, status);
                let complete = result.is_ok();
                if let Err(result) = reply.send(result)
                    && let Err(error) = result
                {
                    log::warn!("Unobserved storage shutdown failure: {error:#}");
                }
                if complete {
                    return;
                }
                deadline = Instant::now() + RETRY_INTERVAL;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                if let Err(error) = flush_pending(&*store, &mut pending, status) {
                    log::warn!("Storage owner dropped with unacknowledged writes: {error:#}");
                }
                return;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
        if matches!(mode, FlushMode::Timed) && Instant::now() >= deadline {
            let result = flush_pending(&*store, &mut pending, status);
            deadline = Instant::now()
                + if result.is_ok() {
                    WRITE_INTERVAL
                } else {
                    RETRY_INTERVAL
                };
        }
    }
}
