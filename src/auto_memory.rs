/// Background auto-memory extraction after each turn.
///
/// After every 5 turns, collects unprocessed user/assistant messages since the
/// last successful extraction boundary and uses the active LLM backend to
/// extract durable facts, then writes them to the MemoryStore (JSON companion
/// file + LanceDB index).
/// No embedding model is required — the JSON file stores full content
/// and LanceDB insertion is best-effort.
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::Notify;

use crate::agent::Message;
use crate::llm::LlmBackend;
use crate::memory_store::{
    MemoryLabel, MemoryScope, MemorySource, MemorySourceSpan, MemoryStore, NewMemoryRecord,
};
use crate::tui::state::TranscriptTurn;

const EXTRACTION_INTERVAL: u64 = 5;
const SHUTDOWN_DRAIN_TIMEOUT: Duration = Duration::from_secs(2);

const EXTRACTION_INSTRUCTION: &str = r#"You are a memory-extraction routine. Read the conversation below and extract durable facts that will be useful for future turns. Output one fact per line, plain text, no markdown bullets. If nothing is worth remembering, output nothing. Focus on: decisions made, constraints discovered, preferences stated, and technical context that is likely to persist across sessions."#;

fn truncate(s: &str, max_chars: usize) -> String {
    if s.len() <= max_chars {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_chars);
        format!("{}…", &s[..end])
    }
}

#[derive(Clone)]
struct AutoMemoryRequest {
    session_id: String,
    thread_id: String,
    baseline_completed_turns: u64,
    completed_turns: u64,
    transcript_turns: Vec<TranscriptTurn>,
    backend: Arc<dyn LlmBackend>,
    store: Arc<MemoryStore>,
}

#[derive(Clone, PartialEq, Eq)]
struct AutoMemoryRequestKey {
    session_id: String,
    completed_turns: u64,
}

impl AutoMemoryRequest {
    fn key(&self) -> AutoMemoryRequestKey {
        AutoMemoryRequestKey {
            session_id: self.session_id.clone(),
            completed_turns: self.completed_turns,
        }
    }
}

#[derive(Default)]
struct AutoMemoryState {
    last_completed_turns_by_session: HashMap<String, u64>,
    current: Option<AutoMemoryRequestKey>,
    pending: Option<AutoMemoryRequest>,
}

/// Process-local auto-memory orchestration state.
///
/// Turn completion only notifies this service. The service runs at most one
/// extraction at a time, coalesces newer eligible snapshots into one trailing
/// request, and exposes a bounded drain hook for shutdown.
struct AutoMemoryService {
    state: Mutex<AutoMemoryState>,
    idle_notify: Notify,
}

impl AutoMemoryService {
    fn new() -> Self {
        Self {
            state: Mutex::new(AutoMemoryState::default()),
            idle_notify: Notify::new(),
        }
    }

    fn notify_turn_completed(
        self: &Arc<Self>,
        app: &crate::tui::state::TuiApp,
        agent: &crate::agent::Agent,
    ) {
        let session_id = agent.session_id.clone();
        let completed_turns = app.committed_turns.len() as u64;
        if completed_turns == 0 || completed_turns % EXTRACTION_INTERVAL != 0 {
            return;
        }

        let request = {
            let mut state = self
                .state
                .lock()
                .expect("auto-memory state lock should not be poisoned");
            let last_completed_turns = state
                .last_completed_turns_by_session
                .get(&session_id)
                .copied()
                .unwrap_or_default();
            if completed_turns <= last_completed_turns {
                return;
            }
            if state.current.as_ref().is_some_and(|current| {
                current.session_id == session_id && current.completed_turns >= completed_turns
            }) {
                return;
            }
            if state.pending.as_ref().is_some_and(|pending| {
                pending.session_id == session_id && pending.completed_turns >= completed_turns
            }) {
                return;
            }

            let request = AutoMemoryRequest {
                session_id: session_id.clone(),
                thread_id: session_id.clone(),
                baseline_completed_turns: last_completed_turns,
                completed_turns,
                transcript_turns: collect_turn_window(app, last_completed_turns, completed_turns),
                backend: agent.llm_backend.clone(),
                store: agent.memory_store.clone(),
            };

            if request.transcript_turns.is_empty() {
                return;
            }

            if state.current.is_some() {
                state.pending = Some(request);
                return;
            }

            state.current = Some(request.key());
            request
        };

        self.spawn_worker(request);
    }

    fn spawn_worker(self: &Arc<Self>, request: AutoMemoryRequest) {
        let service = Arc::clone(self);
        tokio::spawn(async move {
            service.run_worker(request).await;
        });
    }

    async fn run_worker(self: Arc<Self>, mut request: AutoMemoryRequest) {
        let mut guard = WorkerGuard::new(Arc::clone(&self));
        loop {
            let effective_start = self.last_completed_turns(&request.session_id);
            let success = process_request(&request, effective_start).await;
            let next = self.finish_request(&request, success);
            match next {
                Some(next_request) => request = next_request,
                None => {
                    guard.disarm();
                    return;
                }
            }
        }
    }

    fn finish_request(
        self: &Arc<Self>,
        request: &AutoMemoryRequest,
        success: bool,
    ) -> Option<AutoMemoryRequest> {
        let mut state = self
            .state
            .lock()
            .expect("auto-memory state lock should not be poisoned");
        if success {
            let last_completed_turns = state
                .last_completed_turns_by_session
                .entry(request.session_id.clone())
                .or_default();
            *last_completed_turns = (*last_completed_turns).max(request.completed_turns);
        }
        let next = state.pending.take();
        if let Some(ref next_request) = next {
            state.current = Some(next_request.key());
        } else {
            state.current = None;
            self.idle_notify.notify_waiters();
        }
        next
    }

    fn recover_worker_exit(self: &Arc<Self>) {
        let next = {
            let mut state = self
                .state
                .lock()
                .expect("auto-memory state lock should not be poisoned");
            if state.current.is_none() {
                return;
            }
            let next = state.pending.take();
            if let Some(ref next_request) = next {
                state.current = Some(next_request.key());
            } else {
                state.current = None;
                self.idle_notify.notify_waiters();
            }
            next
        };
        if let Some(request) = next {
            self.spawn_worker(request);
        }
    }

    fn last_completed_turns(&self, session_id: &str) -> u64 {
        self.state
            .lock()
            .expect("auto-memory state lock should not be poisoned")
            .last_completed_turns_by_session
            .get(session_id)
            .copied()
            .unwrap_or_default()
    }

    fn is_idle(&self) -> bool {
        let state = self
            .state
            .lock()
            .expect("auto-memory state lock should not be poisoned");
        state.current.is_none() && state.pending.is_none()
    }

    async fn drain(&self) {
        loop {
            let notified = self.idle_notify.notified();
            if self.is_idle() {
                return;
            }
            notified.await;
        }
    }

    async fn drain_with_timeout(&self, timeout: Duration) -> bool {
        tokio::time::timeout(timeout, self.drain()).await.is_ok()
    }
}

fn transcript_role_to_message_role(role: &str) -> Option<&'static str> {
    match role {
        "You" | "user" => Some("user"),
        "Agent" | "assistant" => Some("assistant"),
        _ => None,
    }
}

fn collect_turn_window(
    app: &crate::tui::state::TuiApp,
    start_turn_exclusive: u64,
    end_turn_inclusive: u64,
) -> Vec<TranscriptTurn> {
    let start = start_turn_exclusive as usize;
    let end = end_turn_inclusive.min(app.committed_turns.len() as u64) as usize;
    app.committed_turns[start..end].to_vec()
}

fn collect_messages_from_turns(turns: &[TranscriptTurn]) -> Vec<Message> {
    turns
        .iter()
        .flat_map(|turn| &turn.entries)
        .filter_map(|entry| {
            transcript_role_to_message_role(&entry.role).map(|role| Message {
                role: role.to_string(),
                content: serde_json::Value::String(entry.message.clone()),
            })
        })
        .collect()
}

fn request_source_span(
    start_turn_exclusive: u64,
    end_turn_inclusive: u64,
) -> Option<MemorySourceSpan> {
    let start_turn_index = u32::try_from(start_turn_exclusive.saturating_add(1)).ok()?;
    let end_turn_index = u32::try_from(end_turn_inclusive).ok()?;
    Some(MemorySourceSpan {
        start_turn_index,
        end_turn_index,
    })
}

async fn process_request(request: &AutoMemoryRequest, effective_start_turn_exclusive: u64) -> bool {
    let skip_turns = effective_start_turn_exclusive
        .saturating_sub(request.baseline_completed_turns)
        .min(request.transcript_turns.len() as u64) as usize;
    let messages = collect_messages_from_turns(&request.transcript_turns[skip_turns..]);
    if messages.is_empty() {
        return true;
    }
    let source_span = request_source_span(effective_start_turn_exclusive, request.completed_turns);
    let start_turn_index = effective_start_turn_exclusive.saturating_add(1);

    let result = match request
        .backend
        .summarize(&messages, EXTRACTION_INSTRUCTION)
        .await
    {
        Ok(r) => r,
        Err(err) => {
            eprintln!(
                "Warning: auto-memory summarize failed for session {} turns {}-{}: {err}",
                request.session_id, start_turn_index, request.completed_turns
            );
            return false;
        }
    };

    for line in result.lines() {
        let content = line.trim();
        if content.is_empty() {
            continue;
        }

        let record = NewMemoryRecord {
            title: Some(truncate(content, 80)),
            content: format!("- {content}"),
            labels: vec![MemoryLabel::Fact],
            importance: 0.5,
            pinned: false,
            scope: MemoryScope::User,
            source: MemorySource::AutoMemory,
            session_id: Some(request.session_id.clone()),
            thread_id: Some(request.thread_id.clone()),
            source_span: source_span.clone(),
        };
        if let Err(err) = request.store.insert_text_only(record).await {
            eprintln!(
                "Warning: auto-memory insert failed for session {} turns {}-{}: {err}",
                request.session_id, start_turn_index, request.completed_turns
            );
        }
    }

    true
}

struct WorkerGuard {
    service: Arc<AutoMemoryService>,
    active: bool,
}

impl WorkerGuard {
    fn new(service: Arc<AutoMemoryService>) -> Self {
        Self {
            service,
            active: true,
        }
    }

    fn disarm(&mut self) {
        self.active = false;
    }
}

impl Drop for WorkerGuard {
    fn drop(&mut self) {
        if self.active {
            self.service.recover_worker_exit();
        }
    }
}

fn global_auto_memory_service() -> &'static Arc<AutoMemoryService> {
    static SERVICE: OnceLock<Arc<AutoMemoryService>> = OnceLock::new();
    SERVICE.get_or_init(|| Arc::new(AutoMemoryService::new()))
}

pub async fn drain_auto_memory_for_shutdown() -> bool {
    global_auto_memory_service()
        .drain_with_timeout(SHUTDOWN_DRAIN_TIMEOUT)
        .await
}

#[cfg(test)]
fn maybe_auto_memory_with_service(
    app: &crate::tui::state::TuiApp,
    agent: &crate::agent::Agent,
    service: &Arc<AutoMemoryService>,
) {
    service.notify_turn_completed(app, agent);
}

#[cfg(test)]
fn collect_recent_messages(
    app: &crate::tui::state::TuiApp,
    start_turn_exclusive: u64,
    end_turn_inclusive: u64,
) -> Vec<Message> {
    app.committed_turns
        .get(start_turn_exclusive as usize..end_turn_inclusive as usize)
        .map(collect_messages_from_turns)
        .unwrap_or_default()
}

/// Hook called from tasks.rs after every completed turn.
/// Checks if enough turns have passed and spawns background extraction.
pub fn maybe_auto_memory(app: &crate::tui::state::TuiApp, agent: &crate::agent::Agent) {
    global_auto_memory_service().notify_turn_completed(app, agent);
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    use anyhow::Result;
    use rara_memory::vectordb::VectorDB;
    use rara_tools::tool::ToolManager;
    use tempfile::tempdir;
    use tokio::sync::Notify;

    use super::*;
    use crate::agent::Agent;
    use crate::config::ConfigManager;
    use crate::llm::{ContentBlock, LlmResponse, TokenUsage};
    use crate::session::SessionManager;
    use crate::tui::state::{TranscriptEntry, TranscriptTurn, TuiApp};
    use crate::workspace::WorkspaceMemory;

    #[derive(Default)]
    struct CountingSummaryBackend {
        summarize_calls: AtomicUsize,
    }

    #[async_trait::async_trait]
    impl LlmBackend for CountingSummaryBackend {
        async fn ask(
            &self,
            _messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "unused".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage::default()),
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.0; 8])
        }

        async fn summarize(&self, messages: &[Message], _instruction: &str) -> Result<String> {
            self.summarize_calls.fetch_add(1, Ordering::SeqCst);
            let combined = messages
                .iter()
                .filter_map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join(" | ");
            Ok(format!("remembered: {combined}"))
        }
    }

    fn build_test_app(config_path: &std::path::Path) -> TuiApp {
        TuiApp::new(ConfigManager {
            path: config_path.to_path_buf(),
        })
        .expect("build tui app")
    }

    fn build_test_agent(
        temp: &tempfile::TempDir,
        backend: Arc<dyn LlmBackend>,
    ) -> (Agent, Arc<MemoryStore>) {
        let workspace_root = temp.path().join("workspace");
        let rara_dir = workspace_root.join(".rara");
        std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
        std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
        std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

        let workspace = Arc::new(WorkspaceMemory::from_paths(
            workspace_root.clone(),
            rara_dir.clone(),
        ));
        let session_manager = Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        });
        let agent = Agent::new(
            ToolManager::new(),
            backend,
            Arc::new(VectorDB::new(
                &rara_dir.join("lancedb").display().to_string(),
            )),
            session_manager,
            workspace,
        );
        let store = agent.memory_store.clone();
        (agent, store)
    }

    fn transcript_turn(user: &str, assistant: &str) -> TranscriptTurn {
        TranscriptTurn {
            thinking_duration: None,
            entries: vec![
                TranscriptEntry::new("You", user),
                TranscriptEntry::new("Agent", assistant),
                TranscriptEntry::new("System", "ignored"),
            ],
        }
    }

    fn build_app_with_turns(config_path: &std::path::Path, turn_count: usize) -> TuiApp {
        let mut app = build_test_app(config_path);
        for idx in 1..=turn_count {
            app.committed_turns.push(transcript_turn(
                &format!("user message {idx}"),
                &format!("assistant message {idx}"),
            ));
        }
        app
    }

    #[derive(Default)]
    struct BlockingSummaryBackend {
        summarize_calls: AtomicUsize,
        observed_batches: Mutex<Vec<String>>,
        first_call_started: Notify,
        release_first_call: Notify,
    }

    #[async_trait::async_trait]
    impl LlmBackend for BlockingSummaryBackend {
        async fn ask(
            &self,
            _messages: &[Message],
            _tools: &[serde_json::Value],
        ) -> Result<LlmResponse> {
            Ok(LlmResponse {
                content: vec![ContentBlock::Text {
                    text: "unused".to_string(),
                }],
                stop_reason: Some("end_turn".to_string()),
                usage: Some(TokenUsage::default()),
            })
        }

        async fn embed(&self, _text: &str) -> Result<Vec<f32>> {
            Ok(vec![0.0; 8])
        }

        async fn summarize(&self, messages: &[Message], _instruction: &str) -> Result<String> {
            let call_idx = self.summarize_calls.fetch_add(1, Ordering::SeqCst);
            let combined = messages
                .iter()
                .filter_map(|message| message.content.as_str())
                .collect::<Vec<_>>()
                .join(" | ");
            self.observed_batches
                .lock()
                .expect("observed batches lock")
                .push(combined.clone());
            if call_idx == 0 {
                self.first_call_started.notify_waiters();
                self.release_first_call.notified().await;
            }
            Ok(format!("remembered: {combined}"))
        }
    }

    #[tokio::test]
    async fn auto_memory_uses_tui_roles_without_runtime_blocking() {
        let temp = tempdir().expect("tempdir");
        let app = build_app_with_turns(&temp.path().join("config.json"), 5);
        let backend = Arc::new(CountingSummaryBackend::default());
        let (agent, store) = build_test_agent(&temp, backend.clone());
        let service = Arc::new(AutoMemoryService::new());

        maybe_auto_memory_with_service(&app, &agent, &service);
        maybe_auto_memory_with_service(&app, &agent, &service);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if store.record_count().await.expect("record count") == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("auto-memory task completed");

        assert_eq!(backend.summarize_calls.load(Ordering::SeqCst), 1);

        let records = store.list_recent(None, 10).await.expect("records");
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].source, MemorySource::AutoMemory);
        assert_eq!(records[0].scope, MemoryScope::User);
        assert_eq!(
            records[0].session_id.as_deref(),
            Some(agent.session_id.as_str())
        );
        assert_eq!(
            records[0].thread_id.as_deref(),
            Some(agent.session_id.as_str())
        );
        assert_eq!(
            records[0].source_span,
            Some(MemorySourceSpan {
                start_turn_index: 1,
                end_turn_index: 5,
            })
        );
        assert!(
            records[0]
                .content
                .contains("remembered: user message 1 | assistant message 1")
        );
        assert!(
            records[0]
                .content
                .contains("user message 5 | assistant message 5")
        );
    }

    #[tokio::test]
    async fn auto_memory_coalesces_newer_boundaries_without_losing_skipped_turns() {
        let temp = tempdir().expect("tempdir");
        let app5 = build_app_with_turns(&temp.path().join("config-5.json"), 5);
        let app10 = build_app_with_turns(&temp.path().join("config-10.json"), 10);
        let app15 = build_app_with_turns(&temp.path().join("config-15.json"), 15);

        let backend = Arc::new(BlockingSummaryBackend::default());
        let (agent, _store) = build_test_agent(&temp, backend.clone());
        let service = Arc::new(AutoMemoryService::new());

        maybe_auto_memory_with_service(&app5, &agent, &service);
        tokio::time::timeout(
            Duration::from_secs(1),
            backend.first_call_started.notified(),
        )
        .await
        .expect("first extraction started");

        maybe_auto_memory_with_service(&app10, &agent, &service);
        maybe_auto_memory_with_service(&app15, &agent, &service);
        backend.release_first_call.notify_waiters();

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if backend.summarize_calls.load(Ordering::SeqCst) == 2 && service.is_idle() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("coalesced extraction completed");

        let observed = backend
            .observed_batches
            .lock()
            .expect("observed batches lock")
            .clone();
        assert_eq!(observed.len(), 2);
        assert!(observed[0].contains("user message 1 | assistant message 1"));
        assert!(observed[0].contains("user message 5 | assistant message 5"));
        assert!(observed[1].contains("user message 6 | assistant message 6"));
        assert!(observed[1].contains("user message 10 | assistant message 10"));
        assert!(observed[1].contains("user message 15 | assistant message 15"));
        assert!(!observed[1].contains("user message 1 | assistant message 1"));
        assert!(!observed[1].contains("user message 5 | assistant message 5"));
    }

    #[tokio::test]
    async fn auto_memory_tracks_success_boundaries_per_session() {
        let temp = tempdir().expect("tempdir");
        let app10 = build_app_with_turns(&temp.path().join("config-10.json"), 10);
        let app5 = build_app_with_turns(&temp.path().join("config-5.json"), 5);
        let backend = Arc::new(CountingSummaryBackend::default());
        let (mut agent, store) = build_test_agent(&temp, backend.clone());
        let service = Arc::new(AutoMemoryService::new());
        let first_session_id = agent.session_id.clone();
        let second_session_id = "session-restored".to_string();

        maybe_auto_memory_with_service(&app10, &agent, &service);
        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if store.record_count().await.expect("record count") == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("first auto-memory task completed");

        agent.session_id = second_session_id.clone();
        maybe_auto_memory_with_service(&app5, &agent, &service);

        tokio::time::timeout(Duration::from_secs(1), async {
            loop {
                if store.record_count().await.expect("record count") == 2 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("second auto-memory task completed");

        assert_eq!(backend.summarize_calls.load(Ordering::SeqCst), 2);

        let records = store.list_recent(None, 10).await.expect("records");
        let first_record = records
            .iter()
            .find(|record| record.session_id.as_deref() == Some(first_session_id.as_str()))
            .expect("first session record");
        assert_eq!(
            first_record.source_span,
            Some(MemorySourceSpan {
                start_turn_index: 1,
                end_turn_index: 10,
            })
        );
        assert_eq!(
            first_record.thread_id.as_deref(),
            Some(first_session_id.as_str())
        );

        let second_record = records
            .iter()
            .find(|record| record.session_id.as_deref() == Some(second_session_id.as_str()))
            .expect("second session record");
        assert_eq!(
            second_record.source_span,
            Some(MemorySourceSpan {
                start_turn_index: 1,
                end_turn_index: 5,
            })
        );
        assert_eq!(
            second_record.thread_id.as_deref(),
            Some(second_session_id.as_str())
        );
    }

    #[tokio::test]
    async fn auto_memory_drain_is_bounded_and_finishes_after_release() {
        let temp = tempdir().expect("tempdir");
        let app = build_app_with_turns(&temp.path().join("config.json"), 5);
        let backend = Arc::new(BlockingSummaryBackend::default());
        let (agent, _store) = build_test_agent(&temp, backend.clone());
        let service = Arc::new(AutoMemoryService::new());

        maybe_auto_memory_with_service(&app, &agent, &service);
        tokio::time::timeout(
            Duration::from_secs(1),
            backend.first_call_started.notified(),
        )
        .await
        .expect("first extraction started");

        assert!(!service.drain_with_timeout(Duration::from_millis(50)).await);
        backend.release_first_call.notify_waiters();
        assert!(service.drain_with_timeout(Duration::from_secs(1)).await);
    }

    #[test]
    fn collect_recent_messages_respects_turn_window() {
        let temp = tempdir().expect("tempdir");
        let app = build_app_with_turns(&temp.path().join("config.json"), 15);

        let messages = collect_recent_messages(&app, 5, 15);
        let combined = messages
            .iter()
            .filter_map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join(" | ");

        assert!(combined.contains("user message 6 | assistant message 6"));
        assert!(combined.contains("user message 15 | assistant message 15"));
        assert!(!combined.contains("user message 1 | assistant message 1"));
        assert!(!combined.contains("user message 5 | assistant message 5"));
    }
}
