# TUI Compact Code Sentences

## Summary

Fixed compact live response rendering so Markdown/prose is no longer split by a
hand-rolled sentence parser before display.

## Background

The compact response view split text on periods before rendering. That made
Markdown code identifiers such as `AnalyzeExec.Next()` and
`MemTracker.AttachTo(...)` look like separate progress bullets, and it relied on
that accidental split to hide some planning chatter.

## Scope

- Stop selecting compact response lines through sentence splitting.
- Keep structured plan block filtering before display.
- Render compact response text through the existing Markdown path.
- Suppress non-structured plan-mode chatter while live exploration events are
  visible.

## Validation

```bash
cargo fmt
cargo test tui::render::cells::helper_tests::compact_live_response_message_keeps_markdown_source_intact -- --nocapture
```

The focused test reached the final link step locally but failed with
`No space left on device` from the linker. CI remains the validation path for
the test binary in this workspace.
