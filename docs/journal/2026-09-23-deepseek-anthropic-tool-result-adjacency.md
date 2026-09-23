# DeepSeek Anthropic Route: Tool-Result Adjacency Fix

## Summary

Fixes a real production 400 hit on `deepseek-flash` via the Anthropic-
compatible route landed in
[deepseek-anthropic-route.md](../features/deepseek-anthropic-route.md):
`messages.N: tool_use ids were found without tool_result blocks immediately
after`, reported from a long-running (555k-token) session at its fifth
agentic turn.

## Root Cause

Anthropic requires every `tool_use` block in an assistant turn to have its
`tool_result` in the literal next message. RARA's internal history isn't
shaped that way: `execute_tool_calls` (`src/agent/execution.rs`) records
one turn's parallel tool results as several separate consecutive `Message`s
(one `tool_result` block each, via `tool_result_message` in
`src/agent/planning.rs`), followed by a trailing runtime continuation
nudge — also its own `user`-role message. `chat/completions` tolerates this
shape (it correlates tool results by id, not position), but
`to_anthropic_messages` (`src/llm/deepseek_anthropic/messages.rs`) was
converting each internal `Message` 1:1 into its own Anthropic message, so
any turn calling more than one tool broke the adjacency rule.

While tracing this, a background research pass surfaced a second, related
but distinct gap: an approval- or plan-exit-interrupted turn can abandon
part of its tool_use batch with no `tool_result` ever recorded for it at
all — not delayed, genuinely never written — because `repair_tool_result_history`
(the general repair pass) only runs at the start of a fresh user query
(`src/agent/runtime.rs`), not on the approval-resume path
(`resume_after_plan_approval_with_feedback_events` /
`reject_pending_plan_approval` in `src/agent/planning.rs` call
`run_agent_loop` directly).

## Fix

- Coalesce runs of consecutive `user` messages into one Anthropic message
  instead of emitting one per internal `Message`.
- Port `chat/completions`' `flush_missing_tool_results` pattern: synthesize
  an `is_error: true` `tool_result` for any `tool_use` id still unresolved
  by the time the next assistant message (or end of history) is reached.

## Review Findings (Copilot, PR #897)

- **Do not coalesce assistant messages.** The first version of this fix
  merged same-role runs generically, including `assistant`. Copilot flagged
  that `to_anthropic_message_content` always places a replayed `thinking`
  block first *within its own message*; merging a later assistant
  message's blocks after an earlier one's would bury that block instead of
  keeping it first, breaking the thinking-signature replay contract from
  `deepseek-anthropic-route.md`. Fixed: only consecutive `user` messages
  are coalesced — coalescing was never needed for `assistant` to fix the
  reported bug in the first place, since one model turn's tool_use blocks
  were always already emitted together in a single assistant `Message`.
  Added a regression test asserting two consecutive assistant messages stay
  separate and the second's thinking block stays first in its own message.
- **Documentation.** Updated `deepseek-anthropic-route.md`'s Contracts
  section (the conversion is no longer a near-identity pass-through — the
  adjacency coalescing and missing-result synthesis are now part of the
  documented contract) and Open Risks (the interrupted-turn data-loss gap
  found while investigating, filed as a cross-cutting agent-loop follow-up,
  not fixed here).

## Verification

- `cargo test --lib`: all pass, including new regression tests for both
  fixed scenarios (parallel tool results split across messages; an
  interrupted turn leaving one tool_use unresolved before the next
  assistant turn) and the assistant-non-coalescing guard.
- `cargo clippy --lib -- -D warnings` and `cargo fmt --check`: clean.
- Reproduced live against the real API through the `print` consumer
  binary: a turn with two parallel tool calls (`list_files` + `read_file`),
  then another with two more (`bash` + `read_file`) — both completed
  cleanly with no 400.

## Known Gap, Not Fixed Here

`resume_after_plan_approval_with_feedback_events` /
`reject_pending_plan_approval` bypass `repair_tool_result_history` on the
approval-resume path, so an interrupted multi-tool-call turn's
earlier-resolved-but-never-persisted tool results are lost outright — the
data, not just the pairing, is gone. This backend's flush stops that from
producing a malformed wire request (it fills in a synthetic error result),
but the underlying loss is a cross-cutting agent-loop issue that would
affect any backend enforcing stricter tool_use/tool_result correlation, not
specific to this route. Filed here for whoever picks up the agent-loop
side; not in scope for this PR.
