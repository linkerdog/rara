# ACP Session Workspace Foundation

## Summary

ACP sessions now retain their requested workspace directory and initialize an
independent RARA runtime on the first prompt. Bash and PTY calls inherit that
session workspace when the model does not provide an explicit working directory.

## Key Decisions

- Do not mutate the process cwd for ACP sessions; a single ACP process may host
  sessions for different workspaces concurrently.
- Route cancellation through the session registry and include the ACP session
  ID in runtime-control provenance.
- Give each session a shared atomic cancellation token so a cancellation request
  interrupts an active prompt without waiting for its runtime mutex. The session
  registry uses a synchronous `RwLock` because map operations never span an
  await point.
- Translate assistant text, reasoning, tool lifecycle and streams, and plan
  snapshots through ACP-native session updates instead of formatting them as
  transcript text.
- Preserve approval, todo, warning/error, cancellation, and completion as
  structured runtime-control events with ACP provenance. ACP v1 has no native
  `SessionUpdate` representation for those event families.

## Validation

```bash
cargo fmt --all --check
cargo check -q
cargo test -q -p rara-tools tool::tests::call_context_retains_workspace_root
cargo test -q runtime_control::tests::agent_plan_and_approval_events_preserve_structured_fields
cargo test -q runtime_event_bus::tests::adapter_events_keep_their_acp_provenance
cargo test -q acp::tests::sessions_keep_distinct_requested_workspaces
cargo test -q acp::tests::plan_events_translate_to_native_acp_updates
cargo test -q acp::tests::cancellation_envelope_targets_the_notified_session
cargo test -q acp::tests::cancellation_token_is_shared_without_waiting_for_runtime
git diff --check
```
