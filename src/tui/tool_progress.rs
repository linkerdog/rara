//! Invocation-scoped, incrementally sanitized display tails for tool progress.

use rara_tools::tool::ToolOutputStream;
use uuid::Uuid;

use super::display_sanitize::sanitize_display_line;
use super::display_tail::{DisplayTail, TailLimits};
use super::state::{TranscriptEntryPayload, TuiApp};

pub(crate) const BYTE_LIMIT: usize = 16 * 1024;
const LINE_LIMIT: usize = 16;
const LABEL_LIMIT: usize = 256;
const TRUNCATION: &str = "... live output truncated ...\n";

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ProgressSource {
    pub call_id: Option<String>,
    pub name: String,
    pub stream: ToolOutputStream,
}

pub(crate) enum ProgressCompletion<'a> {
    CallId(&'a str),
    LegacyName(&'a str),
}

#[derive(Clone, Debug)]
pub struct ToolProgressTranscriptPayload {
    pub(crate) id: Uuid,
}

#[derive(Debug)]
struct ProgressBuffer {
    id: Uuid,
    source: ProgressSource,
    tail: DisplayTail,
    header: String,
}

impl ProgressBuffer {
    fn new(source: ProgressSource) -> Self {
        let mut label = sanitize_display_line(&source.name);
        let mut end = label.len().min(LABEL_LIMIT);
        while !label.is_char_boundary(end) {
            end -= 1;
        }
        label.truncate(end);
        let stream = match source.stream {
            ToolOutputStream::Stdout => "stdout",
            ToolOutputStream::Stderr => "stderr",
        };
        let header = format!("{label} {stream}:\n");
        // Reserve chrome before ingestion so the complete entry fits the cap.
        let tail = DisplayTail::new(TailLimits {
            bytes: BYTE_LIMIT - header.len() - TRUNCATION.len() - 1,
            lines: LINE_LIMIT - 2,
        });
        Self {
            id: Uuid::new_v4(),
            source,
            tail,
            header,
        }
    }

    fn push_delta(&mut self, chunk: &str) -> Option<String> {
        if !self.tail.push_delta(chunk) {
            return None;
        }
        let text = self.tail.text();
        let text = text.trim_end();
        if text.trim().is_empty() {
            return None;
        }
        let mut message = self.header.clone();
        if self.tail.is_truncated() {
            message.push_str(TRUNCATION);
        }
        message.push_str(text);
        message.push('\n');
        Some(message)
    }
}

#[derive(Debug, Default)]
pub(crate) struct ToolProgressState {
    sources: Vec<ProgressBuffer>,
}

impl ToolProgressState {
    fn push_delta(&mut self, source: ProgressSource, chunk: &str) -> Option<(Uuid, String)> {
        let index = self
            .sources
            .iter()
            .position(|buffer| buffer.source == source)
            .unwrap_or_else(|| {
                self.sources.push(ProgressBuffer::new(source));
                self.sources.len() - 1
            });
        let buffer = &mut self.sources[index];
        buffer.push_delta(chunk).map(|message| (buffer.id, message))
    }

    pub(crate) fn finish(&mut self, completion: ProgressCompletion<'_>) {
        self.sources.retain(|buffer| match completion {
            ProgressCompletion::CallId(id) => buffer.source.call_id.as_deref() != Some(id),
            ProgressCompletion::LegacyName(name) => {
                buffer.source.call_id.is_some() || buffer.source.name != name
            }
        });
    }
}

pub(crate) fn append_tool_progress(app: &mut TuiApp, source: ProgressSource, chunk: &str) -> bool {
    let Some((id, message)) = app.tool_progress.push_delta(source, chunk) else {
        return false;
    };
    if let Some(entry) = app.active_turn.entries.iter_mut().find(|entry| {
        matches!(&entry.payload, Some(TranscriptEntryPayload::ToolProgress(payload)) if payload.id == id)
    }) {
        entry.message = message;
    } else {
        app.push_entry("Tool Progress", message);
        if let Some(entry) = app.active_turn.entries.last_mut() {
            entry.payload = Some(TranscriptEntryPayload::ToolProgress(ToolProgressTranscriptPayload { id }));
        }
    }
    true
}

#[cfg(test)]
pub(crate) fn format_tool_progress(name: &str, stream: ToolOutputStream, chunk: &str) -> String {
    ProgressBuffer::new(ProgressSource {
        call_id: None,
        name: name.into(),
        stream,
    })
    .push_delta(chunk)
    .unwrap_or_default()
}

#[cfg(test)]
#[path = "tool_progress_tests.rs"]
mod tests;
