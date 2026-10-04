use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, ensure};
use serde_json::json;

use super::{RolloutItem, ThreadSnapshot, ThreadStore, format};
use crate::agent::Message;

impl ThreadStore<'_> {
    pub(super) fn load_export_thread(&self, session_id: &str) -> Result<ThreadSnapshot> {
        let turns_path =
            rara_persistence::thread_turn_log::turn_log_path(&self.rollout_root, session_id);
        match std::fs::read_to_string(&turns_path) {
            Ok(contents) => {
                for (index, line) in contents
                    .lines()
                    .enumerate()
                    .filter(|(_, line)| !line.trim().is_empty())
                {
                    serde_json::from_str::<rara_persistence::thread_turn_log::PersistedTurnRecord>(
                        line,
                    )
                    .with_context(|| {
                        format!(
                            "incomplete export source {} at line {}",
                            turns_path.display(),
                            index + 1
                        )
                    })?;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("read export conversation turns"),
        }
        let transcript_path =
            crate::session_transcript::main_transcript_path(&self.rollout_root, session_id);
        if transcript_path.exists() {
            let transcript = crate::session_transcript::load_transcript(&transcript_path)?;
            ensure!(
                transcript.parse_errors == 0,
                "incomplete export source {}: {} invalid records",
                transcript_path.display(),
                transcript.parse_errors
            );
        }
        let mut thread = self.load_thread(session_id)?;
        let turns = thread
            .rollout_items
            .iter()
            .filter_map(|item| match item {
                RolloutItem::Turn(turn) => Some(turn),
                RolloutItem::Compaction(_)
                | RolloutItem::PlanState { .. }
                | RolloutItem::Interaction(_)
                | RolloutItem::PlanLifecycle(_)
                | RolloutItem::SpawnAgent { .. } => None,
            })
            .collect::<Vec<_>>();
        // The model transcript may have been compacted. Committed display turns
        // retain the conversation before compaction and presentation-only clears.
        if !turns.is_empty() {
            thread.history = turns
                .into_iter()
                .flat_map(|turn| &turn.entries)
                .map(|entry| Message {
                    role: match entry.role.as_str() {
                        "You" => "user",
                        "Agent" | "Responding" => "assistant",
                        "Tool" | "Tool Result" | "Tool Error" => "tool",
                        other => other,
                    }
                    .to_owned(),
                    content: json!(entry.message),
                })
                .collect();
        } else {
            // Legacy model history may contain hidden policy and hook context.
            // Visible TUI system notices use display roles in the turn log above.
            thread
                .history
                .retain(|message| !matches!(message.role.as_str(), "system" | "developer"));
        }
        Ok(thread)
    }

    pub(crate) fn export_thread_file(
        &self,
        session_id: &str,
        path: Option<&str>,
    ) -> Result<PathBuf> {
        let thread = self.load_export_thread(session_id)?;
        let path = path.map(PathBuf::from).unwrap_or_else(|| {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            PathBuf::from(format!("conversation-{session_id}-{stamp}.md"))
        });
        let path = if path.is_absolute() {
            path
        } else {
            Path::new(&thread.metadata.cwd).join(path)
        };
        let data = match path.extension().and_then(|ext| ext.to_str()) {
            None | Some("md") => self.export_thread_markdown(session_id)?.into_bytes(),
            Some("json") => {
                let messages = thread
                    .history
                    .iter()
                    .filter_map(|message| {
                        let content = format::render_message_content(&message.content);
                        (!content.is_empty())
                            .then(|| json!({ "role": message.role, "content": content }))
                    })
                    .collect::<Vec<_>>();
                serde_json::to_vec_pretty(&json!({
                    "schema_version": 1,
                    "metadata": {
                        "session_id": thread.metadata.session_id,
                        "title": thread.metadata.title,
                        "workspace": thread.metadata.cwd,
                        "provider": thread.metadata.provider,
                        "model": thread.metadata.model,
                        "created_at": thread.metadata.created_at,
                        "updated_at": thread.metadata.updated_at,
                    },
                    "summary": thread.compaction.summary,
                    "messages": messages,
                }))?
            }
            _ => anyhow::bail!("Usage: /export [path.md|path.json]"),
        };
        let parent = path
            .parent()
            .context("export path has no parent directory")?;
        ensure!(
            parent.is_dir(),
            "export directory does not exist: {}",
            parent.display()
        );
        let mut file = tempfile::NamedTempFile::new_in(parent)
            .with_context(|| format!("create export in {}", parent.display()))?;
        file.write_all(&data).context("write conversation export")?;
        file.as_file()
            .sync_all()
            .context("sync conversation export")?;
        file.persist_noclobber(&path)
            .with_context(|| format!("save export without overwriting {}", path.display()))?;
        Ok(path)
    }
}
