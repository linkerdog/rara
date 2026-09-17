use std::sync::atomic::Ordering;
use std::time::Instant;

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use tokio::sync::mpsc;

use super::state::{
    HelpTab, Overlay, PermissionMode, RunningTask, RuntimePhase, RuntimeSnapshot, TaskKind,
};
use super::testing::TuiHarness;
use crate::agent::{AgentExecutionMode, BashApprovalMode};

fn harness() -> TuiHarness {
    let tui = TuiHarness::new(RuntimeSnapshot::default()).expect("harness");
    tui.app()
        .sandbox_network_access
        .store(false, Ordering::Relaxed);
    tui
}

fn mark_busy(tui: &mut TuiHarness) {
    let (_, receiver) = mpsc::unbounded_channel();
    tui.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle: tokio::spawn(std::future::pending()),
        started_at: Instant::now(),
        next_heartbeat_after_secs: 2,
        cancellation_token: None,
        cancellation_requested: false,
    });
    tui.app_mut()
        .set_runtime_phase(RuntimePhase::SendingPrompt, Some("sending prompt".into()));
}

async fn press(tui: &mut TuiHarness, code: KeyCode) {
    tui.press_key(KeyEvent::new(code, KeyModifiers::NONE))
        .await
        .expect("dispatch");
}

#[test]
fn initial_permission_label_matches_effective_policy() {
    let tui = harness();
    assert_eq!(tui.app().permission_mode_label(), "accept-edits");
}

#[test]
fn configured_network_without_full_access_remains_custom() {
    let tui = harness();
    tui.app()
        .sandbox_network_access
        .store(true, Ordering::Relaxed);
    assert_eq!(tui.app().permission_mode_label(), "custom");
    assert_ne!(tui.app().permission_mode, PermissionMode::FullAccess);
}

#[test]
fn session_shell_override_matches_auto_without_network_or_full_access() {
    let mut tui = harness();
    tui.app_mut().permission_mode = PermissionMode::Custom;
    tui.app_mut().bash_approval_mode = BashApprovalMode::Always;
    assert_eq!(tui.app().permission_mode_label(), "auto");
    assert!(!tui.app().sandbox_network_access.load(Ordering::Relaxed));
    assert_ne!(tui.app().permission_mode, PermissionMode::FullAccess);
}

#[test]
fn custom_plan_policy_matches_read_only_preset() {
    let mut tui = harness();
    tui.app_mut().permission_mode = PermissionMode::Custom;
    tui.app_mut().agent_execution_mode = AgentExecutionMode::Plan;
    assert_eq!(tui.app().permission_mode_label(), "read-only");
}

#[tokio::test]
async fn busy_inspection_preserves_running_phase() {
    for (command, overlay) in [
        ("/help", Overlay::Help(HelpTab::General)),
        ("/context", Overlay::Context),
        ("/permissions", Overlay::PermissionPicker),
        ("/permission", Overlay::PermissionPicker),
    ] {
        let mut tui = harness();
        mark_busy(&mut tui);
        tui.app_mut().bottom_pane.input = command.into();
        press(&mut tui, KeyCode::Enter).await;
        assert_eq!(tui.app().overlay, Some(overlay), "{command}");
        assert_eq!(tui.app().runtime_phase, RuntimePhase::SendingPrompt);
        assert!(tui.app().is_busy());
        tui.expect_no_commands();
        tui.app_mut()
            .bottom_pane
            .running_task
            .take()
            .unwrap()
            .handle
            .abort();
    }
}

#[tokio::test]
async fn busy_palette_explains_disabled_runtime_changes() {
    let mut tui = harness();
    mark_busy(&mut tui);
    tui.app_mut().bottom_pane.input = "/model".into();
    tui.app_mut().open_overlay(Overlay::CommandPalette);
    let screen = tui.screen_text(100, 30);
    assert!(
        screen.contains("Unavailable while a task is running"),
        "{screen}"
    );
    press(&mut tui, KeyCode::Enter).await;
    assert_eq!(tui.app().runtime_phase, RuntimePhase::SendingPrompt);
    tui.expect_no_commands();
    tui.app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap()
        .handle
        .abort();
}

#[test]
fn permission_picker_describes_actual_auto_policy() {
    let mut tui = harness();
    tui.app_mut().open_overlay(Overlay::PermissionPicker);
    let screen = tui.screen_text(100, 30);
    assert!(!screen.contains("Ask Permissions"), "{screen}");
    assert!(screen.contains("Escalation checks remain"), "{screen}");
    assert!(screen.contains("Current: accept-edits"), "{screen}");
}

#[test]
fn unmatched_policy_explains_its_dimensions() {
    let mut tui = harness();
    tui.app_mut().permission_mode = PermissionMode::Custom;
    tui.app_mut().bash_approval_mode = BashApprovalMode::Once;
    tui.app_mut().open_overlay(Overlay::PermissionPicker);
    let screen = tui.screen_text(100, 30);
    assert!(screen.contains("Current: custom"), "{screen}");
    assert!(screen.contains("shell=once"), "{screen}");
    assert!(screen.contains("network=off"), "{screen}");
}

#[tokio::test]
async fn permission_selection_preserves_pending_plan() {
    let mut tui = harness();
    tui.app_mut().show_pending_plan_approval(Some("exit-plan"));
    tui.app_mut().open_overlay(Overlay::PermissionPicker);
    tui.app_mut().permission_picker_idx = 1;
    press(&mut tui, KeyCode::Enter).await;
    assert!(tui.app().has_pending_plan_approval());
    tui.expect_command(super::runtime_port::RuntimeCommand::SetPermissionMode(
        PermissionMode::AcceptEdits,
    ));
}

#[tokio::test]
async fn busy_permission_selection_waits_for_runtime_receipt() {
    let mut tui = harness();
    mark_busy(&mut tui);
    tui.app_mut().open_overlay(Overlay::PermissionPicker);
    tui.app_mut().permission_picker_idx = 3;
    press(&mut tui, KeyCode::Enter).await;
    tui.expect_command(super::runtime_port::RuntimeCommand::SetPermissionMode(
        PermissionMode::FullAccess,
    ));
    assert_eq!(tui.app().permission_mode_label(), "accept-edits");
    assert!(tui.app().pending_permission_mode.is_none());
    assert!(!tui.app().sandbox_network_access.load(Ordering::Relaxed));
    assert_eq!(tui.app().runtime_phase, RuntimePhase::SendingPrompt);
    tui.app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap()
        .handle
        .abort();
}

#[tokio::test]
async fn failed_permission_request_does_not_claim_application() {
    let mut tui = harness();
    tui.disconnect("offline").await;
    tui.app_mut().open_overlay(Overlay::PermissionPicker);
    tui.app_mut().permission_picker_idx = 3;
    let result = tui
        .press_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE))
        .await;
    assert!(result.is_err());
    assert_eq!(tui.app().permission_mode_label(), "accept-edits");
    assert!(tui.app().pending_permission_mode.is_none());
    assert_eq!(tui.app().overlay, Some(Overlay::PermissionPicker));
    tui.expect_no_commands();
}

#[test]
fn pending_permission_and_description_fit_supported_widths() {
    for width in [60, 80, 120] {
        let mut tui = harness();
        tui.app_mut().pending_permission_mode = Some(PermissionMode::FullAccess);
        tui.app_mut().permission_picker_idx = 3;
        tui.app_mut().open_overlay(Overlay::PermissionPicker);
        let screen = tui.screen_text(width, 24);
        for expected in [
            "Current: accept-edits",
            "Pending: full-access",
            "bypass=off",
            "[4] Full access (always allow) (pending)",
            "classifier checks.",
            "Enter apply",
        ] {
            assert!(
                screen.contains(expected),
                "width={width}, missing {expected}:\n{screen}"
            );
        }
    }
}

#[tokio::test]
async fn busy_argument_mutations_and_aliases_share_availability() {
    for command in [
        "/tasks other",
        "/task-list other",
        "/goal pause",
        "/threads",
        "/approval",
    ] {
        let mut tui = harness();
        mark_busy(&mut tui);
        tui.app_mut().bottom_pane.input = command.into();
        press(&mut tui, KeyCode::Enter).await;
        assert!(
            tui.app()
                .bottom_pane
                .notice
                .as_deref()
                .unwrap()
                .contains("Unavailable while a task is running"),
            "{command}"
        );
        assert_eq!(tui.app().runtime_phase, RuntimePhase::SendingPrompt);
        tui.expect_no_commands();
        tui.app_mut()
            .bottom_pane
            .running_task
            .take()
            .unwrap()
            .handle
            .abort();
    }
}

#[tokio::test]
async fn busy_help_shows_same_disabled_reason() {
    let mut tui = harness();
    mark_busy(&mut tui);
    tui.app_mut().bottom_pane.input = "/model".into();
    tui.app_mut().open_overlay(Overlay::Help(HelpTab::Commands));
    let screen = tui.screen_text(100, 30);
    assert!(
        screen.contains("Unavailable while a task is running"),
        "{screen}"
    );
    tui.app_mut()
        .bottom_pane
        .running_task
        .take()
        .unwrap()
        .handle
        .abort();
}
