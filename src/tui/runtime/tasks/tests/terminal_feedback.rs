use super::*;
use crate::config::TerminalNotificationMethod;
use crate::tui::terminal_control::TerminalTarget;
use crate::tui::terminal_feedback::{TerminalFeedback, TitleMode};

#[tokio::test]
async fn query_completion_distinguishes_success_failure_cancel_and_continuation() {
    for outcome in ["success", "failure", "cancel", "continuation"] {
        let temp = tempdir().unwrap();
        let mut app = TuiApp::new(ConfigManager {
            path: temp.path().join("config.json"),
        })
        .unwrap();
        app.config.tui.terminal.notifications = TerminalNotificationMethod::Osc9;
        app.terminal_focused = false;
        let agent = create_test_agent(&temp);
        app.snapshot.session_id = agent.session_id.clone();
        let result = if matches!(outcome, "failure" | "cancel") {
            Err(anyhow::anyhow!("scripted failure"))
        } else {
            Ok(())
        };
        install_completed_query_task(&mut app, agent, result);
        if outcome == "cancel" {
            let control = super::super::QueryTaskControl::new(app.snapshot.session_id.clone());
            control.request_stop(super::super::QueryStopKind::Cancel, &AtomicBool::new(false));
            app.bottom_pane.running_task.as_mut().unwrap().query_control = Some(control);
        }
        if outcome == "continuation" {
            install_runtime_services(&mut app);
            app.queue_follow_up_message("continue");
        }
        let completion = (&mut app.bottom_pane.running_task.as_mut().unwrap().handle)
            .await
            .unwrap();
        let mut slot = None;
        super::super::finish_running_task_if_ready_from_runtime_port(
            &mut app,
            &mut slot,
            Some(Ok(completion)),
            None,
        )
        .await
        .unwrap();
        let mut feedback = TerminalFeedback::new(TitleMode::Disabled, TerminalTarget::Direct);
        let mut bytes = Vec::new();
        feedback.update(&mut app, &mut bytes).unwrap();
        let expected = match outcome {
            "success" => "\x1b]9;RARA: turn complete\x07",
            "failure" => "\x1b]9;RARA: turn failed\x07",
            "cancel" | "continuation" => "",
            _ => unreachable!(),
        };
        assert_eq!(bytes, expected.as_bytes(), "{outcome}");
        if let Some(task) = app.bottom_pane.running_task.take() {
            task.handle.abort();
            assert!(matches!(task.handle.await, Err(error) if error.is_cancelled()));
        }
    }
}
