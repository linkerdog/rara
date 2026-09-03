# TUI Approval Event Stream Cancellation

## Summary

The in-process TUI runtime subscription now retains its broadcast receiver when
an outer event-loop selection cancels a pending receive. Permission navigation
and selection can no longer detach the runtime stream that delivers approval
responses and resumed turn progress.

## Background

The runtime port adapted `broadcast::Receiver` with a state-consuming
`stream::unfold`. Terminal input wins the outer `tokio::select!` while a runtime
receive is pending, which cancels that receive. Cancellation also dropped the
receiver stored inside the unfold future, so the next poll observed a closed
stream and the approval UI appeared unresponsive.

Codex keeps its event receiver outside the selected receive future, and Claude
Code keeps permission requests in an application queue whose callbacks resolve
the pending tool without replacing the UI input loop. RARA follows the same
cancellation-safe ownership rule with `BroadcastStream`.

## Scope

- Use a cancellation-safe broadcast stream adapter for production TUI runtime
  events.
- Keep the scripted TUI runtime adapter on the same contract.
- Cover cancellation followed by a later runtime event.

## Validation

- `cargo test --lib tui::runtime_port::tests::in_process_event_stream_survives_cancelled_receive -- --nocapture`
- `cargo test --lib tui::runtime_port::tests -- --nocapture`
- `cargo check --all-targets`
- `cargo fmt --all -- --check`
- `git diff --check`

## Follow-Ups

None.
