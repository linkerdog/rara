use super::*;

#[test]
fn explicit_progress_entry_groups_preserves_thinking_indentation() {
    let entries = [TranscriptEntry {
        role: "Thinking".into(),
        message: "    let value = 1;\n  aligned note\n\n".into(),
        payload: None,
    }];

    let groups = explicit_progress_entry_groups(entries.iter());

    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].0, ProgressRole::Thinking);
    assert_eq!(
        groups[0].1,
        vec![
            "    let value = 1;".to_string(),
            "  aligned note".to_string()
        ]
    );
}

#[test]
fn committed_turn_cell_keeps_user_summary_and_agent_sections_in_order() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Review this repo".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "list_files .".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "bash cargo check".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "Final recommendation".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let you_idx = rendered.find("Review this repo").unwrap();
    let explored_idx = rendered.find("# Explored").unwrap();
    let ran_idx = rendered.find("# Ran").unwrap();
    let agent_idx = rendered.find("• Final recommendation").unwrap();

    assert!(rendered.contains("List ."));
    assert!(you_idx < explored_idx);
    assert!(explored_idx < ran_idx);
    assert!(ran_idx < agent_idx);
}

#[test]
fn committed_turn_cell_ignores_routine_system_notices() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Review this repo".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "Final recommendation".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "System".into(),
            message: "prompt finished".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• Final recommendation"));
    assert!(!rendered.contains("System"));
    assert!(!rendered.contains("prompt finished"));
}

#[test]
fn committed_turn_cell_renders_materialized_sidecar_sections() {
    let entries = vec![
        TranscriptEntry { role: "You".into(), message: "Review the workspace logic".into(), payload: None },
        TranscriptEntry {
            role: "Exploring".into(),
            message:
                "Delegate repository exploration: inspect instruction discovery\nSub-agent summary: current discovery is hardcoded"
                    .into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Planning".into(),
            message:
                "Delegate plan refinement: generalize instruction discovery\nSub-agent summary: reuse the workspace traversal helper"
                    .into(),
            payload: None,
        },
        TranscriptEntry { role: "Agent".into(), message: "Here is the final recommendation.".into(), payload: None },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let you_idx = rendered.find("Review the workspace logic").unwrap();
    let explored_idx = rendered.find("# Explored").unwrap();
    let planning_idx = rendered.find("# Planned").unwrap();
    let agent_idx = rendered
        .find("• Here is the final recommendation.")
        .unwrap();

    assert!(you_idx < explored_idx);
    assert!(explored_idx < planning_idx);
    assert!(planning_idx < agent_idx);
    assert!(rendered.contains("Sub-agent summary: current discovery is hardcoded"));
    assert!(rendered.contains("Sub-agent summary: reuse the workspace traversal helper"));
}

#[test]
fn committed_turn_cell_keeps_progress_segments_and_terminal_output() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Run the checks".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Thinking".into(),
            message: "I should run the focused test first.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Running".into(),
            message: "Run cargo test active_turn_cell".into(),
            payload: None,
        },
        TranscriptEntry::terminal_event(TerminalEvent::End(TerminalCommandEvent {
            target: TerminalTarget::BackgroundTask,
            id: Some("bash-123".into()),
            status: "completed".into(),
            command: Some("cargo test active_turn_cell".into()),
            exit_code: Some(0),
            output: vec!["running 38 tests".into(), "ok".into()],
            output_path: None,
            is_error: false,
        })),
        TranscriptEntry {
            role: "Agent".into(),
            message: "The focused tests passed.".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let thinking_idx = rendered.find("┊ I should run").unwrap();
    let progress_idx = rendered.find("Run cargo test active_turn_cell").unwrap();
    let terminal_idx = rendered.find("running 38 tests").unwrap();
    let agent_idx = rendered.find("• The focused tests passed.").unwrap();

    assert!(thinking_idx < progress_idx);
    assert!(progress_idx < terminal_idx);
    assert!(terminal_idx < agent_idx);
    assert!(rendered.contains("ok"));
}

#[test]
fn committed_turn_cell_preserves_interleaved_agent_and_progress_output() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Sync the branch".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "First I will sync the branch.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Running".into(),
            message: "Run git rebase main".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "The first conflict is in keymap.rs.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Running".into(),
            message: "Run cargo test tui::keymap".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_agent = rendered.find("• First I will sync the branch.").unwrap();
    let first_running = rendered.find("Run git rebase main").unwrap();
    let second_agent = rendered
        .find("• The first conflict is in keymap.rs.")
        .unwrap();
    let second_running = rendered.find("Run cargo test tui::keymap").unwrap();

    assert_eq!(
        rendered.matches("• First I will sync the branch.").count(),
        1
    );
    assert_eq!(
        rendered
            .matches("• The first conflict is in keymap.rs.")
            .count(),
        1
    );
    assert_eq!(rendered.matches("Run git rebase main").count(), 1);
    assert_eq!(rendered.matches("Run cargo test tui::keymap").count(), 1);
    assert!(first_agent < first_running);
    assert!(first_running < second_agent);
    assert!(second_agent < second_running);
}

#[test]
fn committed_turn_cell_appends_adjacent_progress_entries() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Inspect the split".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Exploring".into(),
            message: "Read src/context/assembler.rs".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Exploring".into(),
            message: "Read src/context/mod.rs".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Running".into(),
            message: "Run cargo test active_turn_cell".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Running".into(),
            message: "Run cargo check".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Planning".into(),
            message: "Refine the follow-up plan".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Planning".into(),
            message: "Split the UI status work".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let first_explored = rendered.find("Read src/context/assembler.rs").unwrap();
    let second_explored = rendered.find("Read src/context/mod.rs").unwrap();
    let first_ran = rendered.find("Run cargo test active_turn_cell").unwrap();
    let second_ran = rendered.find("Run cargo check").unwrap();
    let first_planned = rendered.find("Refine the follow-up plan").unwrap();
    let second_planned = rendered.find("Split the UI status work").unwrap();

    assert_eq!(rendered.matches("# Explored").count(), 1);
    assert_eq!(rendered.matches("# Ran").count(), 1);
    assert_eq!(rendered.matches("# Planned").count(), 1);
    assert!(first_explored < second_explored);
    assert!(second_explored < first_ran);
    assert!(first_ran < second_ran);
    assert!(second_ran < first_planned);
    assert!(first_planned < second_planned);
}

#[test]
fn committed_turn_cell_places_completion_records_before_final_agent_message() {
    let entries = vec![
        TranscriptEntry { role: "You".into(), message: "Inspect the repo and decide whether to run the migration".into(), payload: None },
        TranscriptEntry { role: "Exploring".into(), message: "└ Read crates/instructions/src/workspace.rs".into(), payload: None },
        TranscriptEntry { role: "Shell Approval Completed".into(), message: "Bash approval: Approved once for command: bash ./scripts/migrate.sh"
                .into(), payload: None },
        TranscriptEntry {
            role: "Agent".into(),
            message:
                "I approved the one-off shell step and can now continue with the final recommendation."
                    .into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let explored_idx = rendered.find("# Explored").unwrap();
    let approval_idx = rendered.find("# Shell Approval Completed").unwrap();
    let agent_idx = rendered
        .find("• I approved the one-off shell step")
        .unwrap();

    assert!(explored_idx < approval_idx);
    assert!(approval_idx < agent_idx);
}

#[test]
fn committed_turn_cell_orders_completion_records_by_interaction_kind() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Inspect the workflow and capture the decision trail".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Question Answered".into(),
            message: "Captured the generic answer.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Shell Approval Completed".into(),
            message: "Approved the one-off shell command.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Planning Question Answered".into(),
            message: "Chose the plan_agent option.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "Here is the final narrative summary.".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    let shell_idx = rendered.find("# Shell Approval Completed").unwrap();
    let planning_question_idx = rendered.find("# Planning Question Answered").unwrap();
    let generic_question_idx = rendered.find("# Question Answered").unwrap();
    let agent_idx = rendered
        .find("• Here is the final narrative summary.")
        .unwrap();

    assert!(shell_idx < planning_question_idx);
    assert!(planning_question_idx < generic_question_idx);
    assert!(generic_question_idx < agent_idx);
    assert!(!rendered.contains("# Plan Decision"));
}

#[test]
fn committed_turn_cell_renders_terminal_result_as_terminal_cell() {
    let entries = vec![
        TranscriptEntry { role: "You".into(), message: "Run tests in the background".into(), payload: None },
        TranscriptEntry { role: "Tool".into(), message: "background_task_status bash-123".into(), payload: None },
        TranscriptEntry { role: "Tool Result".into(), message: "background task bash-123 completed: cargo test\nexit_code: 0\noutput:\ncompile\nrunning tests\nok".into(), payload: None },
        TranscriptEntry { role: "Agent".into(), message: "The background test task completed.".into(), payload: None },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Ran background cargo test"));
    assert!(rendered.contains("└ compile"));
    assert!(rendered.contains("running tests"));
    assert!(rendered.contains("ok"));
    assert!(rendered.contains("• The background test task completed."));
    assert!(!rendered.contains("background_task_status bash-123"));
}

#[test]
fn committed_turn_cell_renders_terminal_result_with_inline_output_path() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Run tests in the background".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool Result".into(),
            message:
                "background task bash-123 running\noutput: /tmp/rara/background-tasks/bash-123.log"
                    .into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Running background bash-123"));
    assert!(rendered.contains("/tmp/rara/background-tasks/bash-123.log"));
    assert!(!rendered.contains("background bash-123 running"));
}

#[test]
fn committed_turn_cell_renders_typed_terminal_event_as_terminal_cell() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Run tests in the background".into(),
            payload: None,
        },
        TranscriptEntry::terminal_event(TerminalEvent::End(TerminalCommandEvent {
            target: TerminalTarget::BackgroundTask,
            id: Some("bash-123".into()),
            status: "completed".into(),
            command: Some("cargo test".into()),
            exit_code: Some(0),
            output: vec!["compile".into(), "running tests".into(), "ok".into()],
            output_path: None,
            is_error: false,
        })),
        TranscriptEntry {
            role: "Agent".into(),
            message: "The background test task completed.".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("Ran background cargo test"));
    assert!(rendered.contains("└ compile"));
    assert!(rendered.contains("running tests"));
    assert!(rendered.contains("ok"));
    assert!(rendered.contains("• The background test task completed."));
    assert!(!rendered.contains("Terminal Event"));
}

#[test]
fn committed_turn_cell_keeps_final_agent_response_when_system_notice_arrives_after_tool_turn() {
    let entries = vec![
        TranscriptEntry {
            role: "You".into(),
            message: "Inspect the repository and summarize the result".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Tool".into(),
            message: "bash cargo check".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "Agent".into(),
            message: "The repository is healthy and the check passed.".into(),
            payload: None,
        },
        TranscriptEntry {
            role: "System".into(),
            message: "Waiting for device-code confirmation.".into(),
            payload: None,
        },
    ];

    let rendered = CommittedTurnCell::new(entries.as_slice(), Some(Path::new(".")))
        .display_lines(100)
        .into_iter()
        .map(|line| line.to_string())
        .collect::<Vec<_>>()
        .join("\n");

    assert!(rendered.contains("• The repository is healthy and the check passed."));
    assert!(!rendered.contains("Waiting for device-code confirmation."));
}
