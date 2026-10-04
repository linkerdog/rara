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

#[tokio::test]
async fn blocked_exit_flush_can_be_cancelled_and_retried_without_losing_writes() {
    use rara_state::state_db::StateDb;

    let mut fixture = Fixture::new().await;
    let db = Arc::new(StateDb::new_for_root_dir(fixture._dir.path().join("storage")).unwrap());
    let root = db.rollout_root();
    let app = fixture.controller.app_mut();
    app.snapshot.session_id = "exit-thread".into();
    app.attach_state_db(db);
    app.push_entry(MessageRole::User, "Keep this turn across a slow exit.");
    let (release, wait) = std::sync::mpsc::channel();
    let (entered, ready) = tokio::sync::oneshot::channel();
    let blocked = app
        .storage
        .as_ref()
        .unwrap()
        .read(move || {
            entered.send(()).unwrap();
            wait.recv()?;
            Ok(())
        })
        .unwrap();
    ready.await.unwrap();
    app.bottom_pane.input = "/quit".into();
    let input = fixture.input.clone();
    let screen = fixture.screen.clone();
    tokio::time::pause();
    {
        let future = fixture.run();
        tokio::pin!(future);
        assert!(poll!(&mut future).is_pending());
        input
            .send(Ok(Event::Key(KeyEvent::new(
                KeyCode::Enter,
                KeyModifiers::NONE,
            ))))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        advance(Duration::from_millis(20)).await;
        assert!(poll!(&mut future).is_pending());
        assert!(
            screen
                .borrow()
                .parser
                .screen()
                .contents()
                .contains("Saving session before exit")
        );
        input
            .send(Ok(Event::Key(KeyEvent::new(
                KeyCode::Esc,
                KeyModifiers::NONE,
            ))))
            .unwrap();
        input
            .send(Ok(Event::Key(KeyEvent::new(
                KeyCode::Char('x'),
                KeyModifiers::NONE,
            ))))
            .unwrap();
        assert!(poll!(&mut future).is_pending());
        release.send(()).unwrap();
        blocked.await.unwrap().unwrap();
        let quit = Event::Key(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
        input.send(Ok(quit.clone())).unwrap();
        input.send(Ok(quit)).unwrap();
        // The real storage thread supplies readiness; virtual time must not race it.
        tokio::time::resume();
        tokio::time::timeout(Duration::from_secs(5), &mut future)
            .await
            .unwrap()
            .unwrap();
    }
    fixture
        .controller
        .app_mut()
        .shutdown_storage()
        .await
        .unwrap();
    let turns = rara_persistence::thread_turn_log::load_turn_records(&root, "exit-thread").unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(
        turns[0].entries[0].message,
        "Keep this turn across a slow exit."
    );
    assert!(rara_persistence::thread_turn_log::load_live_entries(&root, "exit-thread").is_empty());
}
