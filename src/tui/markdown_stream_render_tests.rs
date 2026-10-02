use std::sync::Arc;

use ratatui::{buffer::Buffer, layout::Rect};

use crate::runtime_control::{
    AssistantEvent, RuntimeControlEvent, RuntimeEvent, RuntimeProvenance, SessionEvent,
};
use crate::tui::controller::TuiController;
use crate::tui::custom_terminal::Frame;
use crate::tui::runtime_port::RuntimeProjectionEvent;
use crate::tui::state::{RuntimeSnapshot, TuiApp};
use crate::tui::testing::FakeRuntimeClient;

fn event(sequence: u64, event: RuntimeEvent) -> RuntimeProjectionEvent {
    RuntimeProjectionEvent::Runtime(Box::new(RuntimeControlEvent {
        event_id: format!("stream-event-{sequence}"),
        provenance: RuntimeProvenance::local_tui("stream-test-session"),
        turn_id: Some("stream-test-turn".into()),
        sequence,
        event,
    }))
}

fn paint(controller: &mut TuiController) -> String {
    let area = Rect::new(0, 0, 100, 40);
    let mut buffer = Buffer::empty(area);
    let mut frame = Frame {
        cursor_position: None,
        viewport_area: area,
        buffer: &mut buffer,
    };
    crate::tui::render::render(&mut frame, controller.app_mut());
    (0..area.height)
        .map(|y| {
            (0..area.width)
                .map(|x| buffer[(x, y)].symbol())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[tokio::test]
async fn controller_stream_retains_events_and_holds_table_until_final_render() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(crate::config::ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    app.push_entry("You", "Show the streamed table.");
    let port = Arc::new(FakeRuntimeClient::new(RuntimeSnapshot::default()));
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut controller = TuiController::new(app, port, receiver);
    assert!(
        controller.apply_runtime_event(event(1, RuntimeEvent::Session(SessionEvent::TurnStarted)))
    );
    let chunks = [
        "Visible introduction.\n\n",
        "| StreamHeader | StreamValue |\n",
        "| --- | --- |\n",
        "| long cell content | final value |\n",
        "\nVisible final paragraph.",
    ];
    let mut expected = String::new();
    for (index, chunk) in chunks.iter().enumerate() {
        expected.push_str(chunk);
        assert!(controller.apply_runtime_event(event(
            index as u64 + 2,
            RuntimeEvent::Assistant(AssistantEvent::TextDelta((*chunk).into())),
        )));
    }
    let stream = controller.app().agent_markdown_stream.as_ref().unwrap();
    assert_eq!(stream.raw_text, expected);
    assert_eq!(stream.markdown_work().parses, 0);
    let live = paint(&mut controller);
    assert!(live.contains("Visible introduction."));
    assert!(!live.contains("StreamHeader"));
    assert!(!live.contains("Visible final paragraph."));
    let work = controller
        .app()
        .agent_markdown_stream
        .as_ref()
        .unwrap()
        .markdown_work();
    assert_eq!(paint(&mut controller), live);
    assert_eq!(
        controller
            .app()
            .agent_markdown_stream
            .as_ref()
            .unwrap()
            .markdown_work(),
        work
    );

    assert!(controller.apply_runtime_event(event(
        7,
        RuntimeEvent::Assistant(AssistantEvent::Text(expected)),
    )));
    assert!(controller.apply_runtime_event(event(
        8,
        RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
    )));
    assert!(controller.apply_runtime_event(RuntimeProjectionEvent::Completed { reason: None }));
    let final_text = paint(&mut controller);
    assert!(final_text.contains("StreamHeader"));
    assert!(final_text.contains("long cell content"));
    assert!(final_text.contains("Visible final paragraph."));
    assert!(!final_text.contains("| --- | --- |"), "{final_text}");
    assert!(!controller.app().has_agent_stream());
}

#[tokio::test]
async fn controller_thinking_stream_reuses_rows_and_commits_once_before_text() {
    let temp = tempfile::tempdir().unwrap();
    let mut app = TuiApp::new(crate::config::ConfigManager {
        path: temp.path().join("config.json"),
    })
    .unwrap();
    app.push_entry("You", "Explain the result.");
    let port = Arc::new(FakeRuntimeClient::new(RuntimeSnapshot::default()));
    let (_sender, receiver) = tokio::sync::mpsc::unbounded_channel();
    let mut controller = TuiController::new(app, port, receiver);
    assert!(
        controller.apply_runtime_event(event(1, RuntimeEvent::Session(SessionEvent::TurnStarted)))
    );
    let chunks = ["Checking the **source**.\n\n", "The evidence agrees."];
    for (index, chunk) in chunks.iter().enumerate() {
        assert!(controller.apply_runtime_event(event(
            index as u64 + 2,
            RuntimeEvent::Assistant(AssistantEvent::ThinkingDelta((*chunk).into())),
        )));
    }
    assert_eq!(
        controller
            .app()
            .agent_thinking_stream
            .as_ref()
            .unwrap()
            .markdown_work()
            .parses,
        0
    );
    let live = paint(&mut controller);
    assert!(live.contains("Checking the source."), "{live}");
    assert!(live.contains("The evidence agrees."), "{live}");
    let work = controller
        .app()
        .agent_thinking_stream
        .as_ref()
        .unwrap()
        .markdown_work();
    assert_eq!(paint(&mut controller), live);
    assert_eq!(
        controller
            .app()
            .agent_thinking_stream
            .as_ref()
            .unwrap()
            .markdown_work(),
        work
    );
    for (sequence, chunk) in [(4, "Final "), (5, "answer.")] {
        assert!(controller.apply_runtime_event(event(
            sequence,
            RuntimeEvent::Assistant(AssistantEvent::TextDelta(chunk.into())),
        )));
    }
    assert!(!controller.app().has_agent_thinking_stream());
    let thinking: Vec<_> = controller
        .app()
        .active_turn
        .entries
        .iter()
        .filter(|entry| entry.role == "Thinking")
        .collect();
    assert_eq!(thinking.len(), 1);
    assert_eq!(thinking[0].message, chunks.concat());
    assert!(paint(&mut controller).contains("Final answer."));
    assert!(controller.apply_runtime_event(event(
        6,
        RuntimeEvent::Assistant(AssistantEvent::Text("Final answer.".into())),
    )));
    assert!(controller.apply_runtime_event(event(
        7,
        RuntimeEvent::Session(SessionEvent::TurnFinished { reason: None }),
    )));
    assert!(controller.apply_runtime_event(RuntimeProjectionEvent::Completed { reason: None }));
    assert!(paint(&mut controller).contains("Final answer."));
    assert!(!controller.app().has_agent_stream());
}
