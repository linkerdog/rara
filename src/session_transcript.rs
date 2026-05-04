use std::fs;
use std::io::{BufRead, BufReader, BufWriter, ErrorKind, Write};
use std::path::{Path, PathBuf};

use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::agent::Message;
use crate::atomic_file;

const TRANSCRIPT_FILE_NAME: &str = "transcript.jsonl";
const TRANSCRIPT_SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionTranscriptEntry {
    SessionMeta {
        schema_version: u32,
        session_id: String,
        parent_session_id: Option<String>,
        agent_id: Option<String>,
        is_sidechain: bool,
    },
    Message {
        message_id: String,
        parent_message_id: Option<String>,
        session_id: String,
        agent_id: Option<String>,
        is_sidechain: bool,
        role: String,
        content: Value,
    },
    SpawnAgent {
        event_id: String,
        session_id: String,
        child_session_id: Option<String>,
        agent_id: Option<String>,
        name: Option<String>,
        status: String,
        summary: Option<String>,
    },
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionTranscriptLoad {
    pub entries: Vec<SessionTranscriptEntry>,
    pub parse_errors: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptScope {
    pub session_id: String,
    pub parent_session_id: Option<String>,
    pub agent_id: Option<String>,
    pub is_sidechain: bool,
}

impl TranscriptScope {
    pub fn main(session_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            parent_session_id: None,
            agent_id: None,
            is_sidechain: false,
        }
    }

    pub fn sidechain(
        parent_session_id: impl Into<String>,
        agent_id: impl Into<String>,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            session_id: session_id.into(),
            parent_session_id: Some(parent_session_id.into()),
            agent_id: Some(agent_id.into()),
            is_sidechain: true,
        }
    }
}

pub fn main_transcript_path(root_dir: &Path, session_id: &str) -> PathBuf {
    root_dir.join(session_id).join(TRANSCRIPT_FILE_NAME)
}

pub fn subagent_transcript_path(
    root_dir: &Path,
    parent_session_id: &str,
    agent_id: &str,
) -> PathBuf {
    root_dir
        .join(parent_session_id)
        .join("subagents")
        .join(format!("agent-{}.jsonl", sanitize_path_component(agent_id)))
}

pub fn write_history_snapshot(
    root_dir: &Path,
    session_id: &str,
    history: &[Message],
) -> Result<()> {
    let scope = TranscriptScope::main(session_id);
    write_message_snapshot(&main_transcript_path(root_dir, session_id), &scope, history)
}

pub fn write_message_snapshot(
    path: &Path,
    scope: &TranscriptScope,
    messages: &[Message],
) -> Result<()> {
    let entries = entries_for_messages(scope, messages);
    write_entries_atomic(path, &entries)
}

pub fn append_entry(path: &Path, entry: &SessionTranscriptEntry) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let mut writer = BufWriter::new(file);
    serde_json::to_writer(&mut writer, entry)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    writer.into_inner()?.sync_all()?;
    Ok(())
}

pub fn load_transcript(path: &Path) -> Result<SessionTranscriptLoad> {
    if !path.exists() {
        return Ok(SessionTranscriptLoad::default());
    }
    let file = fs::File::open(path)?;
    let reader = BufReader::new(file);
    let mut entries = Vec::new();
    let mut parse_errors = 0usize;
    let mut line_bytes = Vec::new();
    loop {
        line_bytes.clear();
        match reader.read_until(b'\n', &mut line_bytes) {
            Ok(0) => break,
            Ok(_) => {}
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) => return Err(err.into()),
        }
        let line = match std::str::from_utf8(&line_bytes) {
            Ok(line) => line,
            Err(_) => {
                parse_errors = parse_errors.saturating_add(1);
                continue;
            }
        };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<SessionTranscriptEntry>(line) {
            Ok(entry) => entries.push(entry),
            Err(_) => parse_errors = parse_errors.saturating_add(1),
        }
    }
    Ok(SessionTranscriptLoad {
        entries,
        parse_errors,
    })
}

pub fn model_visible_messages(entries: &[SessionTranscriptEntry]) -> Vec<Message> {
    entries
        .iter()
        .filter_map(|entry| match entry {
            SessionTranscriptEntry::Message {
                is_sidechain: false,
                role,
                content,
                ..
            } => Some(Message {
                role: role.clone(),
                content: content.clone(),
            }),
            _ => None,
        })
        .collect()
}

fn entries_for_messages(
    scope: &TranscriptScope,
    messages: &[Message],
) -> Vec<SessionTranscriptEntry> {
    let mut entries = Vec::with_capacity(messages.len().saturating_add(1));
    entries.push(SessionTranscriptEntry::SessionMeta {
        schema_version: TRANSCRIPT_SCHEMA_VERSION,
        session_id: scope.session_id.clone(),
        parent_session_id: scope.parent_session_id.clone(),
        agent_id: scope.agent_id.clone(),
        is_sidechain: scope.is_sidechain,
    });
    for (idx, message) in messages.iter().enumerate() {
        entries.push(SessionTranscriptEntry::Message {
            message_id: message_id(idx),
            parent_message_id: idx.checked_sub(1).map(message_id),
            session_id: scope.session_id.clone(),
            agent_id: scope.agent_id.clone(),
            is_sidechain: scope.is_sidechain,
            role: message.role.clone(),
            content: message.content.clone(),
        });
    }
    entries
}

fn write_entries_atomic(path: &Path, entries: &[SessionTranscriptEntry]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let tmp_path = path.with_extension(format!("jsonl.tmp-{}", uuid::Uuid::new_v4()));
    let result = (|| -> Result<()> {
        let file = fs::File::create(&tmp_path)?;
        let mut writer = BufWriter::new(file);
        for entry in entries {
            serde_json::to_writer(&mut writer, entry)?;
            writer.write_all(b"\n")?;
        }
        writer.flush()?;
        writer.into_inner()?.sync_all()?;
        atomic_file::replace_file(&tmp_path, path)?;
        Ok(())
    })();
    if let Err(err) = result {
        let _ = fs::remove_file(&tmp_path);
        return Err(err);
    }
    Ok(())
}

fn message_id(idx: usize) -> String {
    format!("msg-{idx:06}")
}

fn sanitize_path_component(value: &str) -> String {
    if value.is_empty() {
        "agent".to_string()
    } else {
        urlencoding::encode(value).into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use anyhow::Result;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        SessionTranscriptEntry, TranscriptScope, append_entry, load_transcript,
        main_transcript_path, model_visible_messages, subagent_transcript_path,
        write_history_snapshot, write_message_snapshot,
    };
    use crate::agent::Message;

    #[test]
    fn history_snapshot_writes_typed_jsonl_with_parent_links() -> Result<()> {
        let temp = tempdir()?;
        let history = vec![
            Message {
                role: "user".to_string(),
                content: json!([{"type": "text", "text": "hello"}]),
            },
            Message {
                role: "assistant".to_string(),
                content: json!([{"type": "text", "text": "hi"}]),
            },
        ];

        write_history_snapshot(temp.path(), "session-1", &history)?;

        let load = load_transcript(&main_transcript_path(temp.path(), "session-1"))?;
        assert_eq!(load.parse_errors, 0);
        assert_eq!(load.entries.len(), 3);
        assert!(matches!(
            &load.entries[0],
            SessionTranscriptEntry::SessionMeta {
                session_id,
                is_sidechain: false,
                ..
            } if session_id == "session-1"
        ));
        assert!(matches!(
            &load.entries[1],
            SessionTranscriptEntry::Message {
                message_id,
                parent_message_id: None,
                role,
                ..
            } if message_id == "msg-000000" && role == "user"
        ));
        assert!(matches!(
            &load.entries[2],
            SessionTranscriptEntry::Message {
                message_id,
                parent_message_id: Some(parent),
                role,
                ..
            } if message_id == "msg-000001" && parent == "msg-000000" && role == "assistant"
        ));
        assert_eq!(model_visible_messages(&load.entries), history);
        Ok(())
    }

    #[test]
    fn sidechain_messages_are_not_model_visible_in_parent_context() -> Result<()> {
        let temp = tempdir()?;
        let main_history = vec![Message {
            role: "user".to_string(),
            content: json!("parent"),
        }];
        let sidechain_history = vec![Message {
            role: "assistant".to_string(),
            content: json!("sidechain details"),
        }];

        write_history_snapshot(temp.path(), "session-1", &main_history)?;
        write_message_snapshot(
            &subagent_transcript_path(temp.path(), "session-1", "review/worker"),
            &TranscriptScope::sidechain("session-1", "review/worker", "child-session"),
            &sidechain_history,
        )?;

        let main = load_transcript(&main_transcript_path(temp.path(), "session-1"))?;
        let sidechain = load_transcript(&subagent_transcript_path(
            temp.path(),
            "session-1",
            "review/worker",
        ))?;
        assert_eq!(model_visible_messages(&main.entries), main_history);
        assert!(model_visible_messages(&sidechain.entries).is_empty());
        assert!(matches!(
            &sidechain.entries[0],
            SessionTranscriptEntry::SessionMeta {
                parent_session_id: Some(parent),
                agent_id: Some(agent_id),
                is_sidechain: true,
                ..
            } if parent == "session-1" && agent_id == "review/worker"
        ));
        Ok(())
    }

    #[test]
    fn append_entry_keeps_malformed_lines_non_fatal_on_load() -> Result<()> {
        let temp = tempdir()?;
        let path = main_transcript_path(temp.path(), "session-1");
        append_entry(
            &path,
            &SessionTranscriptEntry::SpawnAgent {
                event_id: "spawn-1".to_string(),
                session_id: "session-1".to_string(),
                child_session_id: Some("child-1".to_string()),
                agent_id: Some("worker".to_string()),
                name: Some("worker".to_string()),
                status: "completed".to_string(),
                summary: Some("done".to_string()),
            },
        )?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"not json\n")?;

        let load = load_transcript(&path)?;
        assert_eq!(load.entries.len(), 1);
        assert_eq!(load.parse_errors, 1);
        Ok(())
    }

    #[test]
    fn load_transcript_counts_non_utf8_lines_as_parse_errors() -> Result<()> {
        let temp = tempdir()?;
        let path = main_transcript_path(temp.path(), "session-1");
        append_entry(
            &path,
            &SessionTranscriptEntry::SpawnAgent {
                event_id: "spawn-1".to_string(),
                session_id: "session-1".to_string(),
                child_session_id: Some("child-1".to_string()),
                agent_id: Some("worker".to_string()),
                name: Some("worker".to_string()),
                status: "completed".to_string(),
                summary: Some("done".to_string()),
            },
        )?;
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)?
            .write_all(b"\xff\xfe\xfd\n")?;
        append_entry(
            &path,
            &SessionTranscriptEntry::SpawnAgent {
                event_id: "spawn-2".to_string(),
                session_id: "session-1".to_string(),
                child_session_id: Some("child-2".to_string()),
                agent_id: Some("worker".to_string()),
                name: Some("worker".to_string()),
                status: "completed".to_string(),
                summary: Some("done again".to_string()),
            },
        )?;

        let load = load_transcript(&path)?;

        assert_eq!(load.entries.len(), 2);
        assert_eq!(load.parse_errors, 1);
        Ok(())
    }

    #[test]
    fn subagent_paths_preserve_distinct_agent_ids() {
        let temp = tempdir().expect("tempdir");
        let slash_path = subagent_transcript_path(temp.path(), "session-1", "review/worker");
        let colon_path = subagent_transcript_path(temp.path(), "session-1", "review:worker");

        assert_ne!(slash_path, colon_path);
        assert!(slash_path.ends_with("subagents/agent-review%2Fworker.jsonl"));
        assert!(colon_path.ends_with("subagents/agent-review%3Aworker.jsonl"));
    }
}
