use std::path::Path;

use anyhow::Result;
use rara_state::state_db::StateDb;

use crate::memory_store::{MemoryLabel, MemoryRecord};
use crate::session::SessionManager;
use crate::thread_store::{ThreadSnapshot, ThreadStore, ThreadSummary};

pub(crate) fn run_threads_command(limit: usize) -> Result<()> {
    let session_manager = SessionManager::new()?;
    let state_db = StateDb::new()?;
    let store = ThreadStore::new(&session_manager, &state_db);
    let threads = store.list_recent_threads(limit)?;
    print!("{}", format_recent_threads(&threads, limit));
    Ok(())
}

pub(crate) fn run_thread_command(thread_id: &str) -> Result<()> {
    let session_manager = SessionManager::new()?;
    let state_db = StateDb::new()?;
    let store = ThreadStore::new(&session_manager, &state_db);
    let snapshot = store.load_thread(thread_id)?;
    print!("{}", format_thread_snapshot(&snapshot));
    Ok(())
}

pub(crate) fn run_fork_command(thread_id: &str) -> Result<()> {
    let session_manager = SessionManager::new()?;
    let state_db = StateDb::new()?;
    let store = ThreadStore::new(&session_manager, &state_db);
    let forked_thread_id = store.fork_thread(thread_id)?;
    print!(
        "Forked thread {thread_id} -> {forked_thread_id}\nUse `rara resume {forked_thread_id}` to continue the forked thread.\n"
    );
    Ok(())
}

pub(crate) fn format_distilled_memories(thread_id: &str, memories: &[MemoryRecord]) -> String {
    if memories.is_empty() {
        return format!("No durable memories distilled from thread {thread_id}.\n");
    }

    let mut lines = vec![format!(
        "Distilled {} memories from thread {thread_id}:",
        memories.len()
    )];
    for memory in memories {
        lines.push(format!(
            "- {}  importance={:.1}  labels={}  id={}",
            memory.title,
            memory.importance,
            memory
                .labels
                .iter()
                .map(memory_label_name)
                .collect::<Vec<_>>()
                .join(","),
            memory.id
        ));
    }
    format!("{}\n", lines.join("\n"))
}

fn memory_label_name(label: &MemoryLabel) -> &'static str {
    match label {
        MemoryLabel::Insight => "insight",
        MemoryLabel::Decision => "decision",
        MemoryLabel::Fact => "fact",
        MemoryLabel::Procedure => "procedure",
        MemoryLabel::Experience => "experience",
    }
}

fn format_recent_threads(threads: &[ThreadSummary], limit: usize) -> String {
    if threads.is_empty() {
        return "No persisted threads found.\n".to_string();
    }

    let mut lines = Vec::new();
    lines.push(format!("Recent threads (showing up to {limit}):"));
    for thread in threads {
        lines.extend(format_thread_summary_lines(thread));
        lines.push(String::new());
    }
    lines.push("Use `rara thread <THREAD_ID>` to inspect a thread.".to_string());
    lines.push("Use `rara resume <THREAD_ID>` to continue a thread in the TUI.".to_string());
    lines.push("Use `rara resume --last` to continue the newest persisted thread.".to_string());
    format!("{}\n", lines.join("\n"))
}

fn format_thread_summary_lines(thread: &ThreadSummary) -> Vec<String> {
    let workspace = workspace_label(&thread.metadata.cwd);
    let preview = if thread.preview.is_empty() {
        "(no preview)"
    } else {
        thread.preview.as_str()
    };
    vec![
        format!(
            "{}  {} / {}  branch={}  mode={}  origin={}",
            thread.metadata.session_id,
            thread.metadata.provider,
            thread.metadata.model,
            thread.metadata.branch,
            thread.metadata.agent_mode,
            thread.metadata.origin_kind
        ),
        format!(
            "  workspace={}  created_at={}  updated_at={}  history={}  transcript={}  compact={}",
            workspace,
            thread.metadata.created_at,
            thread.metadata.updated_at,
            thread.metadata.history_len,
            thread.metadata.transcript_len,
            thread.compaction.compaction_count
        ),
        format!("  {preview}"),
    ]
}

fn format_thread_snapshot(thread: &ThreadSnapshot) -> String {
    let workspace = workspace_label(&thread.metadata.cwd);
    let turn_count = thread
        .rollout_items
        .iter()
        .filter(|item| matches!(item, crate::thread_store::RolloutItem::Turn(_)))
        .count();
    let interaction_count = thread.interactions.len();
    let plan_step_count = thread.plan_steps.len();
    let recent_files = if thread.compaction.recent_files.is_empty() {
        "-".to_string()
    } else {
        thread.compaction.recent_files.join(", ")
    };

    format!(
        concat!(
            "Thread {}\n",
            "provider={}\n",
            "model={}\n",
            "base_url={}\n",
            "branch={}\n",
            "workspace={}\n",
            "mode={}\n",
            "approval={}\n",
            "origin={}\n",
            "forked_from={}\n",
            "created_at={}\n",
            "updated_at={}\n",
            "history_messages={}\n",
            "transcript_entries={}\n",
            "turns={}\n",
            "plan_steps={}\n",
            "interactions={}\n",
            "compactions={}\n",
            "compaction_before_tokens={}\n",
            "compaction_after_tokens={}\n",
            "compaction_boundary_version={}\n",
            "compaction_recent_files={}\n"
        ),
        thread.metadata.session_id,
        thread.metadata.provider,
        thread.metadata.model,
        thread.metadata.base_url.as_deref().unwrap_or("-"),
        thread.metadata.branch,
        workspace,
        thread.metadata.agent_mode,
        thread.metadata.bash_approval,
        thread.metadata.origin_kind,
        thread
            .metadata
            .forked_from_thread_id
            .as_deref()
            .unwrap_or("-"),
        thread.metadata.created_at,
        thread.metadata.updated_at,
        thread.history.len(),
        thread.metadata.transcript_len,
        turn_count,
        plan_step_count,
        interaction_count,
        thread.compaction.compaction_count,
        thread
            .compaction
            .before_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        thread
            .compaction
            .after_tokens
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        thread
            .compaction
            .boundary_version
            .map(|value| value.to_string())
            .unwrap_or_else(|| "-".to_string()),
        recent_files,
    )
}

fn workspace_label(cwd: &str) -> &str {
    Path::new(cwd)
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("-")
}

#[cfg(test)]
mod tests {
    use rara_state::state_db::PersistedInteraction;

    use super::{format_distilled_memories, format_recent_threads, format_thread_snapshot};
    use crate::memory_store::{MemoryLabel, MemoryRecord, MemoryScope, MemorySource};
    use crate::thread_store::{
        CompactionRecord, RolloutItem, ThreadHistorySource, ThreadMaterializationProvenance,
        ThreadMetadata, ThreadMetadataSource, ThreadNonTurnRolloutSource, ThreadSnapshot,
        ThreadSummary,
    };

    fn metadata() -> ThreadMetadata {
        ThreadMetadata {
            session_id: "thread-123".to_string(),
            cwd: "/tmp/rara".to_string(),
            branch: "main".to_string(),
            provider: "codex".to_string(),
            model: "gpt-5".to_string(),
            base_url: Some("https://chatgpt.com/backend-api/codex".to_string()),
            agent_mode: "build".to_string(),
            bash_approval: "on-request".to_string(),
            created_at: 1_713_955_100,
            origin_kind: "fresh".to_string(),
            forked_from_thread_id: None,
            history_len: 7,
            transcript_len: 4,
            updated_at: 1_713_955_200,
        }
    }

    #[test]
    fn recent_threads_output_mentions_inspect_and_resume_flows() {
        let output = format_recent_threads(
            &[ThreadSummary {
                metadata: metadata(),
                preview: "inspect the repo layout".to_string(),
                compaction: CompactionRecord {
                    compaction_count: 2,
                    ..Default::default()
                },
            }],
            20,
        );

        assert!(output.contains("Recent threads"));
        assert!(output.contains("thread-123  codex / gpt-5"));
        assert!(output.contains("origin=fresh"));
        assert!(output.contains("workspace=rara"));
        assert!(output.contains("Use `rara thread <THREAD_ID>`"));
        assert!(output.contains("Use `rara resume <THREAD_ID>`"));
        assert!(output.contains("Use `rara resume --last`"));
    }

    #[test]
    fn thread_snapshot_output_surfaces_runtime_metadata_and_rollout_counts() {
        let output = format_thread_snapshot(&ThreadSnapshot {
            metadata: metadata(),
            provenance: ThreadMaterializationProvenance {
                metadata_source: ThreadMetadataSource::StateDb,
                history_source: ThreadHistorySource::CanonicalHistory,
                non_turn_rollout_source: ThreadNonTurnRolloutSource::StructuredEventsLog,
            },
            history: vec![],
            compaction: CompactionRecord {
                compaction_count: 3,
                before_tokens: Some(1200),
                after_tokens: Some(400),
                recent_files: vec!["src/main.rs".to_string(), "src/thread_store.rs".to_string()],
                boundary_version: Some(2),
                summary: Some("kept the runtime checkpoint".to_string()),
                ..Default::default()
            },
            plan_explanation: Some("continue thread lifecycle".to_string()),
            plan_steps: vec![],
            interactions: vec![PersistedInteraction {
                kind: "approval".to_string(),
                status: "completed".to_string(),
                title: "Approval".to_string(),
                summary: "approved".to_string(),
                payload: None,
            }],
            rollout_items: vec![RolloutItem::Interaction(PersistedInteraction {
                kind: "approval".to_string(),
                status: "completed".to_string(),
                title: "Approval".to_string(),
                summary: "approved".to_string(),
                payload: None,
            })],
        });

        assert!(output.contains("Thread thread-123"));
        assert!(output.contains("provider=codex"));
        assert!(output.contains("origin=fresh"));
        assert!(output.contains("forked_from=-"));
        assert!(output.contains("workspace=rara"));
        assert!(output.contains("interactions=1"));
        assert!(output.contains("compactions=3"));
        assert!(output.contains("compaction_recent_files=src/main.rs, src/thread_store.rs"));
    }

    #[test]
    fn distilled_memories_output_lists_titles_labels_and_ids() {
        let output = format_distilled_memories(
            "thread-123",
            &[MemoryRecord {
                id: "mem-1".to_string(),
                title: "Use thread distillation".to_string(),
                content: "Distill durable thread decisions into memory records.".to_string(),
                labels: vec![MemoryLabel::Decision, MemoryLabel::Procedure],
                importance: 0.8,
                pinned: false,
                source: MemorySource::ThreadDistill,
                scope: MemoryScope::Thread,
                session_id: Some("thread-123".to_string()),
                thread_id: Some("thread-123".to_string()),
                source_span: None,
                created_at_unix_seconds: 1,
                updated_at_unix_seconds: 1,
            }],
        );

        assert!(output.contains("Distilled 1 memories from thread thread-123:"));
        assert!(output.contains(
            "- Use thread distillation  importance=0.8  labels=decision,procedure  id=mem-1"
        ));
    }
}
