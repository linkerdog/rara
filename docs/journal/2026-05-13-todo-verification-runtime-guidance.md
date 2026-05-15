# Todo, Verification, And Sidebar Guidance

## Context

RARA already had the first slice of Claude-style `todo_write` support: session persistence, runtime
events, transcript cards, and `/context`/`/status` reporting. The next gap was behavioral rather
than structural:

- the default prompt did not push hard enough on bugfix verification and sandbox-persistent testing;
- `todo_write` did not teach the model when to use it or how to keep verification visible;
- the wide-screen sidebar still hid todo state behind a generic context summary instead of showing a
  durable working-set preview.

Claude Code spreads those expectations across `TodoWrite`, testing/verification guidance, and the
verification agent. RARA now mirrors the contract in its own locality model.

## Implemented

- Strengthened execute-mode guidance so complex multi-step work must use `todo_write` proactively,
  keep the full working set current, preserve pending verification items, and continue validation
  even when sandbox permissions block the first command.
- Tightened the default testing/validation prompt so bugfixes follow a reproduce-or-characterize ->
  fix -> focused regression test -> nearby side-effect check loop when practical.
- Added stronger always-on tool-workflow guidance that sandbox denials during tests/builds/checks
  are a routing problem to diagnose, narrow, or escalate rather than a reason to stop.
- Expanded the `todo_write` tool description and schema text so the call site itself teaches:
  complex-task usage, full-list replacement semantics, prompt status updates, and verification-aware
  completion discipline.
- Expanded the `bash` tool description so validation commands keep pushing through sandbox denials by
  inspecting the exact failure and requesting escalation only when the sandbox is proven to be the
  blocker.
- Added a bounded `Todo` section to the wide-screen sidebar with progress, active item, and a short
  checklist preview inspired by OpenCode's persistent session todo surface, but adapted to RARA's
  existing left-sidebar layout instead of adding a separate dock.

## Trade-Offs

- RARA still does not enforce verification items structurally inside `todo_write`; the stronger
  contract remains prompt-and-tool-description guidance rather than schema rejection. That keeps the
  tool simple while making the expected workflow much harder for the model to miss.
- The sidebar preview is intentionally bounded to avoid crowding out more stable context. Full todo
  detail stays in `/context` and `/status`.
- Sandbox persistence is expressed in both the always-on prompt and the `bash` tool description. The
  overlap is deliberate because the rule matters both at workflow level and at tool-choice time.

## Validation

- `cargo fmt`
- `cargo test -p rara-instructions`
- `cargo test todo_write`
- `cargo test sidebar`
