# 2026-05-09 Prompt Source Status Audit

## Summary

Audited the prompt-source reporting path and closed the TODO for unifying
`discover_prompt_sources()` with TUI `/status` source reporting.

## Evidence

- `crates/instructions/src/prompt.rs` builds `EffectivePrompt.sources` from
  `discover_prompt_sources(workspace, runtime)`.
- `src/context/runtime.rs` converts the same `EffectivePrompt.sources` into
  `PromptContextView.source_entries`.
- `src/tui/state/mod.rs` copies `runtime_context.prompt.source_entries` into
  `RuntimeSnapshot.prompt_source_entries`.
- `src/tui/command/status.rs` renders `/status` prompt sources exclusively from
  `app.snapshot.prompt_source_entries`.

## Decision

The current `/status` prompt-source surface already reads the shared structured
prompt-source view instead of running a separate discovery path. No code change
is needed for this TODO item.

## Validation

- `rg -n "status_prompt_sources_text|prompt_source_entries|discover_prompt_sources\\(" src/tui src/context src/agent crates/instructions/src -S`
- Existing focused test: `status_prompt_sources_text_includes_structured_entries`
