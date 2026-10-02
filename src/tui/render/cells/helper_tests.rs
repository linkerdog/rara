use super::{
    HistoryCell, RespondingCell, plan::compact_live_response_message,
    plan::compact_live_response_source, plan::parse_render_plan_block,
};

#[test]
fn compact_live_response_message_keeps_markdown_source_intact() {
    let rendered = compact_live_response_message(
        "Let me trace `AnalyzeExec.Next()` including `MemTracker.AttachTo(GlobalAnalyzeMemoryTracker)`. Next I will inspect `select.go`.",
    )
    .unwrap();

    assert_eq!(
        rendered,
        "Let me trace `AnalyzeExec.Next()` including `MemTracker.AttachTo(GlobalAnalyzeMemoryTracker)`.\nNext I will inspect `select.go`."
    );
}

#[test]
fn compact_live_response_message_prefers_first_sentence_and_next_step() {
    let rendered = compact_live_response_message(
        "I inspected the repository structure. I checked the runtime boundary. I checked the prompt assembly path. Next I will inspect the persistence layer. Then I will verify the restore contract.",
    )
    .unwrap();

    assert_eq!(
        rendered,
        "I inspected the repository structure.\nI checked the runtime boundary.\nNext I will inspect the persistence layer."
    );
}

#[test]
fn compact_responding_cell_preserves_line_breaks_and_inline_code() {
    let lines = RespondingCell::from_compact_message(
        "Inspect `AnalyzeExec.Next()`.\nNext I will inspect `select.go`.".to_string(),
        4,
        None,
    )
    .display_lines(100)
    .into_iter()
    .map(|line| line.to_string())
    .collect::<Vec<_>>();

    assert_eq!(
        lines,
        vec![
            "• Inspect AnalyzeExec.Next().".to_string(),
            "• Next I will inspect select.go.".to_string(),
        ]
    );
}

#[test]
fn compact_live_response_source_strips_structured_plan_block() {
    let rendered = compact_live_response_source(
        "I will compare the source and current todo.\nI will tie recommendations to concrete source evidence.\n<proposed_plan>\n- [completed] Inspect runtime entrypoint\n- [pending] Tighten render path\n</proposed_plan>",
    )
    .unwrap();

    assert_eq!(
        rendered,
        "I will compare the source and current todo.\nI will tie recommendations to concrete source evidence."
    );
}

#[test]
fn compact_live_response_source_drops_checklist_tail_after_prose() {
    let rendered = compact_live_response_source(
        "I inspected the current context path.\nI will reuse the existing assembler output.\n- [completed] Review context/runtime.rs\n- [pending] Add a focused test",
    )
    .unwrap();

    assert_eq!(
        rendered,
        "I inspected the current context path.\nI will reuse the existing assembler output."
    );
}

#[test]
fn compact_live_response_source_keeps_prose_after_structured_plan_block() {
    let rendered = compact_live_response_source(
        "I inspected the current context path.\n<proposed_plan>\n- [completed] Review context/runtime.rs\n- [pending] Add a focused test\n</proposed_plan>\nI am starting the focused patch now.",
    )
    .unwrap();

    assert_eq!(
        rendered,
        "I inspected the current context path.\nI am starting the focused patch now."
    );
}

#[test]
fn parse_render_plan_block_extracts_steps_and_explanation() {
    let parsed = parse_render_plan_block(
        "I reviewed the code.\n<proposed_plan>\n- [completed] Inspect the runtime path\n- Tighten the render path\n</proposed_plan>\nKeep the diff narrow.",
    )
    .unwrap();

    assert_eq!(
        parsed,
        (
            vec![
                (
                    "completed".to_string(),
                    "Inspect the runtime path".to_string()
                ),
                ("pending".to_string(), "Tighten the render path".to_string()),
            ],
            Some("Keep the diff narrow.".to_string()),
        )
    );
}
