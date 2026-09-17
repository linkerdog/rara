use super::*;

fn pending_shell_app() -> (tempfile::TempDir, TuiApp) {
    let temp = tempdir().unwrap();
    let mut app = TuiApp::new(ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    let command = format!(
        "git push -u origin feature/long-command | tail -3; gh pr create --title '{}' --body-file /tmp/body.md\n# {}",
        "review the permission interaction contract ".repeat(8),
        "additional command context ".repeat(6),
    );
    app.push_entry("You", "Publish the prepared change.");
    app.snapshot
        .pending_interactions
        .push(PendingInteractionSnapshot {
            kind: InteractionKind::Approval,
            title: "Shell Approval".into(),
            summary: command.clone(),
            options: vec![],
            note: None,
            approval: Some(PendingApprovalSnapshot {
                tool_use_id: "long-shell-command".into(),
                command: command.clone(),
                allow_net: false,
                payload: BashCommandInput {
                    command: Some(command),
                    cwd: Some("/home/developer/workspaces/project".into()),
                    ..Default::default()
                },
            }),
            source: None,
            created_at_epoch_seconds: None,
        });
    (temp, app)
}

#[test]
fn long_shell_command_keeps_every_choice_in_actual_viewport() {
    let (_temp, mut app) = pending_shell_app();
    for (width, rows) in [(180, 28), (80, 24), (60, 14), (40, 10)] {
        for selected in 0..4 {
            app.approval_picker_idx = selected;
            let height = desired_viewport_height(&app, width, rows);
            let screen = render_screen_text(&mut app, width, height);
            for label in [
                "[1] Allow once",
                "[2] Allow prefix",
                "[3] Allow session",
                "[4] Reject",
            ] {
                assert!(
                    screen.contains(label),
                    "{width}x{rows}, missing {label}:\n{screen}"
                );
            }
            assert!(
                screen.contains(&format!("▸ [{}]", selected + 1))
                    || screen.contains(&format!("▸[{}]", selected + 1)),
                "selected choice hidden at {width}x{rows}:\n{screen}"
            );
        }
    }
}

#[test]
fn pending_decision_uses_available_terminal_height() {
    let (_temp, app) = pending_shell_app();
    for rows in [8, 10, 14, 24] {
        assert_eq!(desired_viewport_height(&app, 80, rows), rows);
    }
}

#[test]
fn clipped_command_preview_marks_elision_without_hiding_choices() {
    let (_temp, mut app) = pending_shell_app();
    let screen = render_screen_text(&mut app, 80, 24);
    assert!(screen.contains("details truncated"), "{screen}");
    assert!(screen.contains("[4] Reject"), "{screen}");
}
