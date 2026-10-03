use std::sync::atomic::{AtomicBool, Ordering};

use super::*;
use crate::runtime_control::SessionControlRequest;
use crate::tui::runtime::{QueryStopKind, QueryTaskControl};

#[tokio::test]
async fn confirmed_ctrl_c_and_slash_quit_end_the_actual_loop() {
    for shortcut in [true, false] {
        let mut fixture = Fixture::new().await;
        let input = fixture.input.clone();
        if !shortcut {
            fixture.controller.app_mut().bottom_pane.input = "/quit".into();
        }
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        if shortcut {
            let key = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
            input.send(Ok(key.clone())).unwrap();
            assert!(poll!(&mut future).is_pending());
            input.send(Ok(key)).unwrap();
        } else {
            input
                .send(Ok(Event::Key(KeyEvent::new(
                    KeyCode::Enter,
                    KeyModifiers::NONE,
                ))))
                .unwrap();
        }
        assert!(
            matches!(poll!(&mut future), std::task::Poll::Ready(Ok(()))),
            "quit must exit successfully without EOF"
        );
    }
}

struct TaskDrop(Option<tokio::sync::oneshot::Sender<()>>);

impl Drop for TaskDrop {
    fn drop(&mut self) {
        // The receiver disappearing means the assertion already failed.
        if let Some(sender) = self.0.take() {
            let _ = sender.send(());
        }
    }
}

#[tokio::test]
async fn first_ctrl_c_cancels_query_but_confirmed_quit_aborts_remaining_work() {
    let mut fixture = Fixture::new().await;
    let input = fixture.input.clone();
    let port = fixture.port.clone();
    let commands = fixture.commands.clone();
    let token = Arc::new(AtomicBool::new(false));
    let control = QueryTaskControl::new("loop-query".into());
    let (started, started_rx) = tokio::sync::oneshot::channel();
    let (dropped, mut dropped_rx) = tokio::sync::oneshot::channel();
    let (_sender, receiver) = mpsc::unbounded_channel();
    let handle = tokio::spawn(async move {
        let _drop = TaskDrop(Some(dropped));
        started.send(()).unwrap();
        std::future::pending::<TaskCompletion>().await
    });
    started_rx.await.unwrap();
    fixture.controller.app_mut().bottom_pane.running_task = Some(RunningTask {
        kind: TaskKind::Query,
        receiver,
        handle,
        started_at: std::time::Instant::now(),
        next_heartbeat_after_secs: 100,
        cancellation_token: Some(token.clone()),
        query_control: Some(control.clone()),
    });
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        let key = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        input.send(Ok(key.clone())).unwrap();
        assert!(poll!(&mut future).is_pending());
        let captured = port.commands();
        assert!(matches!(
            captured.as_slice(),
            [RuntimeCommand::Session(
                SessionControlRequest::CancelCurrentTurn
            )]
        ));
        // The fake captures transport commands; deliver through the real processor branch.
        commands.send(captured[0].clone()).unwrap();
        assert!(poll!(&mut future).is_pending());
        assert!(token.load(Ordering::SeqCst));
        assert_eq!(control.stop_kind(), Some(QueryStopKind::Cancel));
        assert!(matches!(
            dropped_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Empty)
        ));
        input.send(Ok(key)).unwrap();
        assert!(matches!(poll!(&mut future), std::task::Poll::Ready(Ok(()))));
    }
    tokio::time::timeout(Duration::from_secs(1), dropped_rx)
        .await
        .expect("confirmed quit must release the running task")
        .unwrap();
    assert!(fixture.controller.app().bottom_pane.running_task.is_none());
}
