use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rara_memory::memory_handle::MemoryHandle;
use rara_state::state_db::{PersistedCompactState, PersistedPromptRuntimeState, StateDb};
use rara_tools::tool::ToolManager;
use serde_json::{Value, json};

use super::{apply_startup_resume, parse_bash_approval_mode, restore_thread_by_id};
use crate::agent::{Agent, AgentOutputMode, BashApprovalMode};
use crate::config::ConfigManager;
use crate::llm::{ContentBlock, LlmBackend, LlmResponse, Message, MockLlm};
use crate::session::SessionManager;
use crate::tui::event_loop::StartupResumeTarget;
use crate::tui::state::{NoticeLevel, TuiApp};
use crate::workspace::WorkspaceMemory;

const THREAD_ID: &str = "approval-thread";
const RECOVERY_WARNING: &str = "Unknown saved bash approval mode; restored suggestion mode.";

#[test]
fn approval_parser_accepts_only_known_values() {
    for (stored, expected) in [
        ("once", Some(BashApprovalMode::Once)),
        ("always", Some(BashApprovalMode::Always)),
        ("suggestion", Some(BashApprovalMode::Suggestion)),
        ("future-mode", None),
        ("", None),
        ("ALWAYS", None),
        (" always", None),
        ("always\n", None),
    ] {
        assert_eq!(parse_bash_approval_mode(stored), expected, "{stored:?}");
    }
}

struct Fixture {
    _dir: tempfile::TempDir,
    app: TuiApp,
    agent: Option<Agent>,
    db: Arc<StateDb>,
}

impl Fixture {
    fn new(mode: &str) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("workspace");
        std::fs::create_dir_all(&root).unwrap();
        let data = dir.path().join("state");
        let sessions = Arc::new(SessionManager::new_for_rara_dir(data.clone()).unwrap());
        sessions
            .save_session(
                THREAD_ID,
                &[Message {
                    role: "user".into(),
                    content: json!("resume this session"),
                }],
            )
            .unwrap();
        let db = Arc::new(StateDb::new_for_root_dir(data.clone()).unwrap());
        db.upsert_session(
            THREAD_ID,
            &root.display().to_string(),
            "main",
            "mock",
            "test",
            None,
            "execute",
            mode,
            None,
            &PersistedPromptRuntimeState::default(),
            1,
            0,
            &PersistedCompactState::default(),
        )
        .unwrap();
        let agent = Agent::new(
            ToolManager::new(),
            Arc::new(MockLlm),
            Arc::new(MemoryHandle::new(
                &data.join("memory").display().to_string(),
            )),
            sessions,
            Arc::new(WorkspaceMemory::from_paths(root, data)),
        );
        let mut app = TuiApp::new(ConfigManager {
            path: dir.path().join("config.json"),
        })
        .unwrap();
        app.attach_state_db(db.clone());
        Self {
            _dir: dir,
            app,
            agent: Some(agent),
            db,
        }
    }

    async fn restore(&mut self) {
        restore_thread_by_id(THREAD_ID, &mut self.app, &mut self.agent)
            .await
            .unwrap();
        self.app.flush_storage().await.unwrap();
    }

    fn assert_mode(&self, expected: BashApprovalMode, persisted: &str) {
        assert_eq!(self.app.snapshot.session_id, THREAD_ID);
        assert_eq!(self.agent.as_ref().unwrap().bash_approval_mode, expected);
        assert_eq!(self.app.bash_approval_mode, expected);
        assert_eq!(
            self.db
                .load_session_runtime_state(THREAD_ID)
                .unwrap()
                .unwrap()
                .bash_approval,
            persisted
        );
    }
}

#[tokio::test]
async fn known_modes_restore_without_recovery_warning() {
    for (stored, expected) in [
        ("once", BashApprovalMode::Once),
        ("always", BashApprovalMode::Always),
        ("suggestion", BashApprovalMode::Suggestion),
    ] {
        let mut fixture = Fixture::new(stored);
        fixture.restore().await;
        fixture.assert_mode(expected, stored);
        assert_eq!(fixture.app.notice().unwrap().level(), NoticeLevel::Info);
        assert_eq!(
            fixture.app.notice_text(),
            Some("Resumed thread approval-thread.")
        );
    }
}

#[tokio::test]
async fn unknown_modes_recover_conservatively_on_both_startup_paths() {
    for stored in [
        "future-mode",
        "",
        "ALWAYS",
        " always",
        "always\n",
        "\u{1b}[2J",
    ] {
        for target in [
            StartupResumeTarget::Latest,
            StartupResumeTarget::ThreadId(THREAD_ID.into()),
        ] {
            let mut fixture = Fixture::new(stored);
            apply_startup_resume(&target, &mut fixture.app, &mut fixture.agent);
            super::loading::finish_restore_for_test(&mut fixture.app, &mut fixture.agent)
                .await
                .unwrap();
            fixture.app.flush_storage().await.unwrap();
            fixture.assert_mode(BashApprovalMode::Suggestion, "suggestion");
            assert_eq!(fixture.app.notice().unwrap().level(), NoticeLevel::Warning);
            assert_eq!(
                fixture.app.notice_text().unwrap(),
                format!("Resumed thread {THREAD_ID}. {RECOVERY_WARNING}")
            );
            assert_eq!(
                fixture
                    .app
                    .active_turn
                    .entries
                    .iter()
                    .filter(|entry| entry.message.contains(RECOVERY_WARNING))
                    .count(),
                1
            );
            fixture.restore().await;
            fixture.assert_mode(BashApprovalMode::Suggestion, "suggestion");
            assert_eq!(fixture.app.notice().unwrap().level(), NoticeLevel::Info);
        }
    }
}

#[tokio::test]
async fn approval_recovery_keeps_other_restore_warnings() {
    let mut fixture = Fixture::new("future-mode");
    fixture
        .db
        .save_goal(THREAD_ID, &json!({ "status": "unknown" }))
        .unwrap();
    fixture.restore().await;
    fixture.assert_mode(BashApprovalMode::Suggestion, "suggestion");
    let notice = fixture.app.notice().unwrap();
    assert_eq!(notice.level(), NoticeLevel::Warning);
    assert!(notice.message().contains("Goal persistence unavailable"));
    assert!(notice.message().contains(RECOVERY_WARNING));
}

#[tokio::test]
async fn approval_recovery_preserves_explicit_full_access() {
    let mut fixture = Fixture::new("future-mode");
    fixture.agent.as_mut().unwrap().set_full_access_mode(true);
    fixture.restore().await;
    fixture.assert_mode(BashApprovalMode::Suggestion, "suggestion");
    assert!(fixture.agent.as_ref().unwrap().full_access_mode);
}

#[tokio::test]
async fn incomplete_live_restore_keeps_entries_and_other_recovery_warnings() {
    let mut fixture = Fixture::new("future-mode");
    let path = fixture.db.rollout_root().join(THREAD_ID).join("live.jsonl");
    std::fs::write(&path, b"{\"role\":\"Agent\",\"message\":\"before corruption\"}\n{invalid}\n{\"role\":\"Agent\",\"message\":\"after corruption\"}\n").unwrap();
    fixture.restore().await;
    let notice = fixture.app.notice().unwrap();
    assert_eq!(notice.level(), NoticeLevel::Warning);
    assert!(notice.message().contains(RECOVERY_WARNING));
    assert!(
        notice
            .message()
            .contains("Live transcript recovery incomplete")
    );
    assert_eq!(
        fixture.app.active_turn.entries[0].message,
        "before corruption"
    );
    assert_eq!(
        fixture.app.active_turn.entries[1].message,
        "after corruption"
    );
    let persisted = std::fs::read_to_string(path).unwrap();
    assert!(persisted.contains("Live transcript recovery incomplete"));
}

#[derive(Default)]
struct BashRequestBackend(AtomicBool);

#[async_trait::async_trait]
impl LlmBackend for BashRequestBackend {
    async fn ask(&self, _messages: &[Message], _tools: &[Value]) -> anyhow::Result<LlmResponse> {
        let first = !self.0.swap(true, Ordering::Relaxed);
        Ok(LlmResponse {
            content: vec![if first {
                ContentBlock::ToolUse {
                    id: "write-request".into(),
                    name: "bash".into(),
                    input: json!({ "command": "git push origin main" }),
                }
            } else {
                ContentBlock::Text {
                    text: "done".into(),
                }
            }],
            stop_reason: Some(if first { "tool_use" } else { "end_turn" }.into()),
            usage: None,
        })
    }

    async fn summarize(&self, _messages: &[Message], _instruction: &str) -> anyhow::Result<String> {
        Ok("summary".into())
    }
}

#[tokio::test]
async fn recovered_mode_keeps_mutating_bash_pending_approval() {
    let mut fixture = Fixture::new("future-mode");
    fixture.restore().await;
    let agent = fixture.agent.as_mut().unwrap();
    agent.llm_backend = Arc::new(BashRequestBackend::default());
    agent
        .query_with_mode("push changes".into(), AgentOutputMode::Silent)
        .await
        .unwrap();
    let approval = agent.pending_approval.as_ref().expect("explicit approval");
    assert_eq!(approval.tool_use_id, "write-request");
    assert_eq!(approval.request.summary(), "git push origin main");
}
