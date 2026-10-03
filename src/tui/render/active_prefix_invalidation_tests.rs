use std::{path::PathBuf, time::Instant};

use crate::{
    agent::AgentExecutionMode,
    tui::{
        message_role::MessageRole,
        queued_input::PendingFollowUpMessage,
        state::{
            AgentMarkdownStreamState, InteractionKind, PendingInteractionSnapshot, RunningTask,
            RuntimePhase, RuntimeSnapshot, TaskKind, TuiApp,
        },
        testing::TuiHarness,
    },
};

fn question() -> PendingInteractionSnapshot {
    PendingInteractionSnapshot {
        kind: InteractionKind::RequestInput,
        title: "Old question".into(),
        summary: String::new(),
        options: vec![("old option".into(), "old description".into())],
        note: None,
        approval: None,
        source: None,
        created_at_epoch_seconds: None,
    }
}

fn assert_refresh_matches_cold(app: &TuiApp, width: u16) {
    let before = app.active_assembly_count.get();
    let actual = super::renderable_transcript_lines(app, width);
    assert_eq!(app.active_assembly_count.get(), before + 1);
    let unchanged = super::renderable_transcript_lines(app, width);
    assert_eq!(app.active_assembly_count.get(), before + 1);
    if !actual.is_empty() {
        assert!(std::ptr::eq(
            actual.get(0).unwrap(),
            unchanged.get(0).unwrap()
        ));
    }
    assert_eq!(
        actual.iter().cloned().collect::<Vec<_>>(),
        super::transcript_cache_tests::canonical_rows(app, width),
    );
}

#[test]
fn nested_prefix_inputs_refresh_without_length_or_edge_fingerprints() {
    let mutations: &[fn(&mut TuiApp)] = &[
        |app| {
            app.active_turn.entries[1]
                .message
                .replace_range(.., "New thought.\n\nAnother thought.")
        },
        |app| {
            app.active_turn.entries[1]
                .message
                .replace_range(.., "**thought.**\n\nAnother thought.")
        },
        |app| app.active_turn.entries[1].role = MessageRole::Planning,
        |app| {
            app.active_live
                .exploration_actions
                .push("Read a changed file".into())
        },
        |app| app.snapshot.cwd = "/changed/workspace".into(),
        |app| app.snapshot.plan_steps[0].1.replace_range(.., "new step"),
        |app| app.snapshot.plan_explanation = Some("New explanation".into()),
        |app| {
            app.snapshot.pending_interactions[0]
                .title
                .replace_range(.., "New question")
        },
        |app| {
            app.snapshot.pending_interactions[0].options[0]
                .0
                .replace_range(.., "new option")
        },
        |app| {
            app.bottom_pane.pending_follow_up_messages[0]
                .text
                .replace_range(.., "new pending")
        },
        |app| app.bottom_pane.queued_follow_up_messages[0].replace_range(.., "new queued"),
        |app| app.bottom_pane.pending_planning_suggestion = Some("Make a plan".into()).into(),
        |app| {
            app.runtime_phase_detail
                .set_if_changed(Some("New detail".into()))
        },
        |app| app.runtime_phase = RuntimePhase::RunningTool,
        |app| app.agent_execution_mode = AgentExecutionMode::Plan,
        |app| app.thinking_collapsed = true,
        |app| app.approval_picker_idx = 1,
    ];
    for mutation in mutations {
        let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
        let app = harness.app_mut();
        app.push_entry(MessageRole::User, "Inspect the [source](./src/lib.rs:12).");
        app.push_entry(MessageRole::Thinking, "Old thought.\n\nAnother thought.");
        app.push_entry(MessageRole::Agent, "A final answer.");
        app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
        app.snapshot
            .plan_steps
            .push(("pending".into(), "old step".into()));
        app.snapshot.pending_interactions.push(question());
        app.bottom_pane
            .pending_follow_up_messages
            .push(PendingFollowUpMessage {
                text: "old pending".into(),
                release_after_boundary: 1,
            });
        app.bottom_pane
            .queued_follow_up_messages
            .push("old queued".into());
        super::renderable_transcript_lines(app, 60);
        mutation(app);
        assert_refresh_matches_cold(app, 60);
    }
}

#[test]
fn replacement_reset_width_and_stream_lifecycle_match_cold_rendering() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "First prompt.");
    app.push_entry(MessageRole::Agent, "The old answer.");
    assert_refresh_matches_cold(app, 80);
    let mut replacement = app.active_turn.clone();
    replacement.entries[1].message = "The new answer.".into();
    app.active_turn = replacement;
    assert_refresh_matches_cold(app, 80);
    app.snapshot = RuntimeSnapshot {
        cwd: "/replacement".into(),
        ..Default::default()
    }
    .into();
    assert_refresh_matches_cold(app, 80);
    for width in [8, 1, 0, 80] {
        assert_refresh_matches_cold(app, width);
    }
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_delta("Streaming answer.");
    assert_refresh_matches_cold(app, 80);
    app.finalize_agent_stream(None);
    assert_refresh_matches_cold(app, 80);
    app.finalize_active_turn();
    assert_refresh_matches_cold(app, 80);
    app.restore_committed_turns(Vec::new());
    assert_refresh_matches_cold(app, 80);
}

#[test]
fn thinking_append_and_same_length_replacement_refresh_the_cached_window() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "Think through the evidence.");
    app.set_runtime_phase(RuntimePhase::ProcessingResponse, None);
    app.append_agent_thinking_delta("Old evidence.");
    app.active_live.thinking_started_at = None;
    assert_refresh_matches_cold(app, 80);
    let mutations: &[fn(&mut TuiApp)] = &[
        |app| {
            let mut replacement = AgentMarkdownStreamState::new(PathBuf::from("/workspace"));
            replacement.push_delta("New evidence.");
            app.agent_thinking_stream = Some(replacement);
        },
        |app| app.append_agent_thinking_delta("\n\nMore reasoning."),
        |app| {
            app.agent_thinking_stream
                .as_mut()
                .unwrap()
                .finalize_display_lines()
        },
    ];
    for mutation in mutations {
        mutation(app);
        let before = app.active_assembly_count.get();
        let actual = super::renderable_transcript_lines(app, 80);
        assert_eq!(app.active_assembly_count.get(), before);
        assert_eq!(
            actual.iter().cloned().collect::<Vec<_>>(),
            super::transcript_cache_tests::canonical_rows(app, 80)
        );
    }
    app.append_agent_delta("Final answer.");
    assert_refresh_matches_cold(app, 80);
}

#[tokio::test]
async fn busy_state_without_a_phase_change_refreshes_the_prefix() {
    let mut harness = TuiHarness::new(RuntimeSnapshot::default()).unwrap();
    let app = harness.app_mut();
    app.push_entry(MessageRole::User, "Wait for the response.");
    assert_refresh_matches_cold(app, 80);
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let handle = tokio::spawn(std::future::pending());
    app.bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: Instant::now(),
        next_heartbeat_after_secs: 1,
        cancellation_token: None,
        query_control: None,
    });
    assert_refresh_matches_cold(app, 80);
    app.bottom_pane.running_task.take().unwrap().handle.abort();
    assert_refresh_matches_cold(app, 80);
}
