# Model-aware context budget

## Summary

RARA now derives output reserve and compaction slack from the resolved model
context window instead of applying one fixed reserve pattern to every model.
Small, medium, and long-context windows keep different margins so compacting
does not start too early on long-context providers or too late on small local
windows.

## Background

The context budget already flowed through provider/model resolution, but
`context_budget_from_window` used one ratio plus one fixed 2048-token slack for
all models.  That made the final threshold less intentional for both 8K local
windows and million-token provider windows.

## Key Decisions

- Small windows reserve a larger fraction for output and keep a small slack.
- Medium windows preserve the previous broad behavior.
- Long windows cap output reserve and use a larger absolute compaction slack.
- The policy remains window-driven and provider-neutral; provider-specific
  overrides still belong in backend model-window resolution.

## Validation

- `cargo fmt`
- `cargo test llm::tests::context_budget_scales_reserved_output_by_window_size -- --nocapture`
- `cargo test llm::tests::derives_context_budget_for_codex_like_models -- --nocapture`
- `cargo test llm::tests::derives_context_budget_for_deepseek_v4_models -- --nocapture`
