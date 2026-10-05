# DeepSeek Anthropic Route: Shared tool_use/tool_result Pairing Primitive

## Summary

Follow-up to
[2026-09-24-deepseek-anthropic-orphaned-tool-results.md](2026-09-24-deepseek-anthropic-orphaned-tool-results.md),
which ended with a process note suggesting unification. This change extracts
the tool_use/tool_result bookkeeping shared by `repair_tool_result_history`
(`src/tool_result/transcript.rs`) and `to_anthropic_messages`
(`src/llm/deepseek_anthropic/messages.rs`) into one primitive module,
`src/tool_result/pairing.rs`, instead of maintaining two independent
implementations of the same rules.

## Motivation

Three fixes in three days landed on `to_anthropic_messages`'s own,
hand-rolled version of the same tracking `repair_tool_result_history`
already did for `agent.history`. Each fix re-derived the same case
(pending-id tracking, filler synthesis, orphan dropping) against a
slightly different shape. Continuing to patch each site independently as
new edge cases surface was flagged as the wrong direction; unifying them
removes the class of bug rather than the latest instance of it.

## Change

`src/tool_result/pairing.rs` (new) holds the shared primitives, operating
directly on Anthropic-shaped content-block arrays (`{"type": "tool_use",
"id", ...}` / `{"type": "tool_result", "tool_use_id", "content", ...}`),
which is also our internal block shape:

- `tool_use_ids_in_blocks` — collect `tool_use` ids from a block array.
- `has_tool_result_block` — whether any block is a `tool_result`.
- `keep_or_drop_tool_results` — keeps every non-`tool_result` block
  unconditionally; keeps a `tool_result` only if its id is currently
  pending (removing it from pending), drops it otherwise.
- `synthetic_tool_result_blocks` — builds one `is_error: true` filler per
  still-pending id, draining pending.

Both `repair_tool_result_history` and `to_anthropic_messages` now delegate
to these functions, keeping only their own turn/message-shape bookkeeping
(one `Message` per turn with no coalescing, vs. Anthropic's coalesced
same-role runs) around the shared core.

## Behavior Change

While unifying, `to_anthropic_messages` picked up a correctness fix:
it previously only flushed pending `tool_use` ids when the *next assistant*
message was reached. `repair_tool_result_history` has always used the
stricter, more correct rule — flush on any message that isn't itself
carrying the matching `tool_result` (a `system` block, or an unrelated
`user` message with no `tool_result` in it, must not leave a `tool_use`
silently pending past it). `to_anthropic_messages` now matches this rule.

## Verification

- `cargo test --lib`: 1503 passed, 0 failed, 1 ignored. Includes the
  pre-existing `repairs_missing_tool_results` test (unchanged behavior for
  `repair_tool_result_history`), 3 new `pairing` unit tests, and a new
  `message_conversion_flushes_pending_tool_use_before_an_unrelated_user_message`
  proving the corrected flush rule.
- `cargo clippy --lib -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- Reproduced live against the real API through the `print` consumer binary:
  parallel `read_file` tool calls in one turn, followed by a single
  `read_file` call in the next turn, then a final answer — no 400.

## Open Risk (unchanged)

The approval-resume paths (`resume_after_plan_approval_with_feedback_events`,
`reject_pending_plan_approval` in `src/agent/planning.rs`) still bypass
`query_inner`'s `repair_tool_result_history` call. Out of scope here; noted
in [deepseek-anthropic-route.md](../features/deepseek-anthropic-route.md).
