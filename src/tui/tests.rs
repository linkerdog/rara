use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind};
use rara_memory::memory_handle::MemoryHandle;
use rara_tools::tool::ToolManager;
use secrecy::ExposeSecret;
use tempfile::tempdir;
use tokio::sync::mpsc;

use super::app_event::AppEvent;
use super::event_dispatch::dispatch_event_with_runtime;
use super::event_stream::{UiEvent, translate_event};
use super::provider_flow::{
    codex_auth_is_available, open_provider_family_overlay, sync_codex_credential_from_auth_store,
};
use crate::agent::{Agent, PendingApproval};
use crate::codex_model_catalog::{CodexModelOption, CodexReasoningOption};
use crate::config::{ConfigManager, OpenAiEndpointKind};
use crate::config::{DEFAULT_CODEX_BASE_URL, DEFAULT_CODEX_CHATGPT_BASE_URL, DEFAULT_CODEX_MODEL};
use crate::llm::MockLlm;
use crate::session::SessionManager;
use crate::tools::bash::BashCommandInput;
use crate::tui::command::palette_commands;
use crate::tui::state::ApiKeyTarget;
use crate::workspace::WorkspaceMemory;

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

fn shifted_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::SHIFT)
}

fn mouse_scroll(kind: MouseEventKind) -> Event {
    Event::Mouse(MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: KeyModifiers::NONE,
    })
}
use super::state::{
    InteractionKind, ListPickerKind, Overlay, PendingApprovalSnapshot, PendingInteractionSnapshot,
    PermissionMode, ProviderFamily, RunningTask, StatusTab, TaskKind, TuiApp,
};
use super::testing::FakeRuntimeClient;
use super::{RuntimeCommand, RuntimeMaintenanceCommand};
use super::{dispatch_event, map_key_to_event};

fn provider_family_idx(family: ProviderFamily) -> usize {
    super::state::PROVIDER_FAMILIES
        .iter()
        .position(|(candidate, _, _)| *candidate == family)
        .expect("provider family present")
}

fn add_pending_shell_approval(app: &mut TuiApp) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Pending Approval".into(),
            summary: "git rebase --continue".into(),
            options: Vec::new(),
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "tool-1".into(),
                command: "git rebase --continue".into(),
                allow_net: false,
                payload: Default::default(),
            }),
            source: None,
            created_at_epoch_seconds: None,
        });
}

fn add_pending_plan_approval(app: &mut TuiApp) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::PlanApproval,
            title: "Plan Ready".into(),
            summary: "Review the plan.".into(),
            options: Vec::new(),
            note: None,
            approval: None,
            source: None,
            created_at_epoch_seconds: None,
        });
}

fn test_agent_for_pending_approval(temp: &tempfile::TempDir) -> Agent {
    let rara_dir = temp.path().join(".rara");
    std::fs::create_dir_all(rara_dir.join("rollouts")).expect("rollouts");
    std::fs::create_dir_all(rara_dir.join("sessions")).expect("sessions");
    std::fs::create_dir_all(rara_dir.join("tool-results")).expect("tool results");

    let mut agent = Agent::new(
        ToolManager::new(),
        Arc::new(MockLlm),
        Arc::new(MemoryHandle::new(
            &rara_dir.join("memory").display().to_string(),
        )),
        Arc::new(SessionManager {
            storage_dir: rara_dir.join("rollouts"),
            legacy_storage_dir: rara_dir.join("sessions"),
        }),
        Arc::new(WorkspaceMemory::from_paths(
            temp.path().join("repo"),
            rara_dir,
        )),
    );
    agent.pending_approval = Some(PendingApproval {
        tool_use_id: "tool-1".to_string(),
        request: BashCommandInput {
            command: Some("git rebase --continue".to_string()),
            ..Default::default()
        },
    });
    agent
}

fn abort_running_task(app: &mut TuiApp) {
    if let Some(task) = app.bottom_pane.running_task.take() {
        task.handle.abort();
    }
}

fn add_pending_request_input(app: &mut TuiApp, option_count: usize) {
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::RequestInput,
            title: "Choose one".into(),
            summary: String::new(),
            options: (1..=option_count)
                .map(|index| (format!("option {index}"), String::new()))
                .collect(),
            note: None,
            approval: None,
            source: None,
            created_at_epoch_seconds: None,
        });
}

mod approvals;
mod catalog_and_mouse;
mod codex_setup;
mod composer_editing;
mod composer_keys;
mod provider_connections;
mod provider_profiles;
mod session_commands;
