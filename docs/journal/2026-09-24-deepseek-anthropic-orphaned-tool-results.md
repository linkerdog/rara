# DeepSeek Anthropic Route: Drop Orphaned Tool Results

## Summary

Fixes the mirror image of the adjacency bug fixed in
[2026-09-23-deepseek-anthropic-tool-result-adjacency.md](2026-09-23-deepseek-anthropic-tool-result-adjacency.md):
a second real production 400 from `api.deepseek.com/anthropic`, this time
`messages.N.content.0: unexpected tool_use_id found in tool_result blocks:
<id>. Each tool_result block must have a corresponding tool_use block in
the previous message`. See the canonical contract in
[deepseek-anthropic-route.md](../features/deepseek-anthropic-route.md).

## Root Cause

The previous fix (`to_anthropic_messages`,
`src/llm/deepseek_anthropic/messages.rs`) ported half of
`repair_tool_result_history`'s (`src/tool_result/transcript.rs`) repair
logic: it synthesizes a filler `tool_result` for any `tool_use` id still
pending when the next assistant message is reached. It did not port the
other half: `repair_tool_result_history` also *drops* any `tool_result`
block whose id is not currently pending, rather than passing it through.
`to_anthropic_messages`'s own `resolve_tool_use_ids` only tracked which ids
were resolved — it never filtered the blocks themselves, so a
`tool_result` for an id that was already resolved, already flushed as a
synthetic filler, or never existed in this history at all still made it
onto the wire, landing in a message whose immediately preceding message
does not contain a matching `tool_use` — the exact rejection reported.

## Fix

Renamed the tracking helper to `resolve_and_filter_tool_results`: it now
returns the filtered block list. Every non-`tool_result` block is kept
unconditionally; a `tool_result` block is kept only if its id is currently
pending (removing it from the pending set), and dropped otherwise —
mirroring `repair_tool_result_history`'s existing, proven behavior for
exactly this case.

## Verification

- `cargo test --lib`: all pass. Added
  `message_conversion_drops_tool_result_with_no_pending_tool_use` (an
  orphaned id gets dropped, non-`tool_result` blocks in the same message
  are kept) and updated `message_conversion_folds_tool_results_into_user_role`
  to include the preceding `tool_use` message its `tool_result` now
  requires to survive.
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check`: clean.
- Reproduced live against the real API through the `print` consumer
  binary: parallel tool calls (`list_files` + `read_file`, then `bash` +
  `read_file`) across two turns, no 400.

## Process Note

This is the third fix in three days to the same conversion function, each
one a different facet of Anthropic's stricter tool_use/tool_result
adjacency requirement (`to_anthropic_messages` was originally written as
a near-identity, per-message conversion — see the original route journal).
The underlying pattern in all three — coalesce same-role runs, synthesize
missing results, drop orphaned ones — is now a reasonably complete mirror
of `repair_tool_result_history`'s own logic, applied at the wire-shape
layer instead of the `Message` history layer. If a fourth variant surfaces,
it is worth considering unifying these two repair passes (or having
`to_anthropic_messages` delegate to a shared repair primitive) rather than
continuing to patch this function's own bespoke bookkeeping case by case.
