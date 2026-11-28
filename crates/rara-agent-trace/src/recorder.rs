use std::fs::{File, OpenOptions, create_dir, create_dir_all};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::{AGENT_TRACE_SCHEMA_VERSION, AgentTraceEvent, TraceManifest, TraceRecord};

/// Paths created for one trace session.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentTraceLocation {
    pub directory: PathBuf,
    pub manifest_path: PathBuf,
    pub events_path: PathBuf,
}

/// Session-scoped recorder for an ordered content-free trace stream.
#[derive(Debug)]
pub struct AgentTraceRecorder {
    inner: Option<TraceWriter>,
}

#[derive(Debug)]
struct TraceWriter {
    session_id: String,
    started_at: Instant,
    location: AgentTraceLocation,
    state: Mutex<TraceWriterState>,
}

#[derive(Debug)]
struct TraceWriterState {
    next_sequence: u64,
    events: BufWriter<File>,
}

impl AgentTraceRecorder {
    /// Return a no-op recorder for runtimes that did not opt into local tracing.
    pub fn disabled() -> Self {
        Self { inner: None }
    }

    /// Create a fresh trace directory under `root` for one stable session identity.
    pub fn new(root: impl AsRef<Path>, session_id: impl Into<String>) -> io::Result<Self> {
        let session_id = session_id.into();
        create_dir_all(root.as_ref())?;
        let created_at_unix_ms = unix_time_ms();
        let directory = create_trace_directory(root.as_ref(), &session_id, created_at_unix_ms)?;

        let manifest_path = directory.join("manifest.json");
        let mut manifest = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&manifest_path)?;
        write_json_line(
            &mut manifest,
            &TraceManifest {
                schema_version: AGENT_TRACE_SCHEMA_VERSION,
                session_id: session_id.clone(),
                created_at_unix_ms,
            },
        )?;
        manifest.flush()?;

        let events_path = directory.join("events.jsonl");
        let events = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&events_path)?;
        Ok(Self {
            inner: Some(TraceWriter {
                session_id,
                started_at: Instant::now(),
                location: AgentTraceLocation {
                    directory,
                    manifest_path,
                    events_path,
                },
                state: Mutex::new(TraceWriterState {
                    next_sequence: 0,
                    events: BufWriter::new(events),
                }),
            }),
        })
    }

    /// Return whether this recorder persists events instead of acting as a no-op.
    pub fn is_enabled(&self) -> bool {
        self.inner.is_some()
    }

    /// Return the trace bundle paths when persistence is enabled.
    pub fn location(&self) -> Option<AgentTraceLocation> {
        self.inner.as_ref().map(|writer| writer.location.clone())
    }

    /// Append one event and flush it so a live diagnostic consumer can read it.
    pub fn record(&self, turn_id: Option<&str>, event: AgentTraceEvent) -> io::Result<()> {
        let Some(writer) = self.inner.as_ref() else {
            return Ok(());
        };
        let mut state = writer.lock()?;
        let sequence = state.next_sequence;
        state.next_sequence = state
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| io::Error::other("agent trace sequence overflow"))?;
        let record = TraceRecord {
            schema_version: AGENT_TRACE_SCHEMA_VERSION,
            sequence,
            timestamp_unix_ms: unix_time_ms(),
            elapsed_ms: elapsed_ms(writer.started_at),
            session_id: writer.session_id.clone(),
            turn_id: turn_id.map(ToString::to_string),
            event,
        };
        write_json_line(&mut state.events, &record)?;
        state.events.flush()
    }
}

impl TraceWriter {
    fn lock(&self) -> io::Result<MutexGuard<'_, TraceWriterState>> {
        self.state
            .lock()
            .map_err(|_| io::Error::other("agent trace writer mutex poisoned"))
    }
}

fn write_json_line<W, T>(writer: &mut W, value: &T) -> io::Result<()>
where
    W: Write,
    T: Serialize,
{
    serde_json::to_writer(&mut *writer, value)
        .map_err(|error| io::Error::other(format!("serialize agent trace record: {error}")))?;
    writer.write_all(b"\n")
}

fn unix_time_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn elapsed_ms(start: Instant) -> u64 {
    start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
}

fn create_trace_directory(
    root: &Path,
    session_id: &str,
    created_at_unix_ms: u64,
) -> io::Result<PathBuf> {
    let prefix = format!(
        "agent-trace-{}-{created_at_unix_ms}-{}",
        safe_path_component(session_id),
        std::process::id(),
    );
    for suffix in 0_u16..=u16::MAX {
        let name = if suffix == 0 {
            prefix.clone()
        } else {
            format!("{prefix}-{suffix}")
        };
        let directory = root.join(name);
        match create_dir(&directory) {
            Ok(()) => return Ok(directory),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "unable to allocate unique agent trace directory",
    ))
}

fn safe_path_component(value: &str) -> String {
    let component = value
        .chars()
        .map(|character| match character {
            'a'..='z' | 'A'..='Z' | '0'..='9' | '-' | '_' => character,
            _ => '_',
        })
        .take(80)
        .collect::<String>();
    if component.is_empty() {
        "session".to_string()
    } else {
        component
    }
}

#[cfg(test)]
#[path = "recorder_tests.rs"]
mod tests;
