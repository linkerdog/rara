# App Server Stdio Protocol Foundation

## Summary

Define the version1 server-first stdio boundary needed by an external process
supervisor. The shared codec, explicit session cursor subscription and their
focused checks are implemented. This does not yet make the app-server command
available or complete a downstream provider integration.

## Background

The canonical runtime already owns sessions, cancellation, ordered in-memory
replay, and shutdown. The shared request enums do not provide wire startup,
correlated acceptance, receipt retention, or a shutdown frame. Wrapping the old
raw-Agent dispatcher would bypass the canonical session owner.

## Key Decisions

- Keep a server-first handshake and concrete upstream-owned wire fixtures.
- Negotiate exact request methods; shared enum variants do not prove capability.
- Separate runtime incarnation, session/turn identity, request IDs and event IDs.
- Bound serialization while writing and use fixed codec errors.
- Advertise process-lifetime replay/receipts and no persistent approval callbacks.
- Reuse the canonical event bus for explicit cursor subscriptions. Preserve
  original event identity after close and report both exhausted and future
  cursors as resynchronization requirements.
- Retain canonical session cleanup results before publishing Closed. Child-tree
  failures remain failures for concurrent and repeated shutdown callers. Host
  cleanup has one independent owner, retains failed sessions and rejects new
  admission while cleanup is pending or failed. Successful cleanup permits
  explicit host reuse. The near-limit actor module was split into command,
  handle and actor responsibilities without changing public import paths.
- Mirror connection gating and bounded ordered output from the inspected Codex
  transport, and correlated/cancelled control responses from the inspected Claude
  Code transport, without adopting either wire schema or implementation.

Reference revisions: Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` and the local
Claude Code checkout `4b9d30f7953273e567a18eb819f4eddd45fcc877`. These are pinned
source observations, not claims about current released versions.

## Validation

The protocol library has16 passing tests, including12 new cases for golden wire
shapes, method/family and lifetime consistency, identity bounds, malformed input,
inclusive frame limits, bounded output allocation and sanitized codec errors.
All-target Clippy passes with warnings denied. Formatting and whitespace checks
pass;11 local documentation targets resolve. Validation used the repository's
`nightly-2026-05-02` toolchain, the locked offline dependency graph and an isolated
temporary target directory. No dependency or Bazel configuration changed.

The runtime-session integration suite has7 passing tests after the cursor
follow-up. It covers retained event identity after close, exhausted/future cursor
rejection, existing same-session serialization, cancellation, provider tool
identity and cross-session concurrency. Root all-target Clippy also passes with
warnings denied. Runtime tests reused a matching-toolchain build cache with
incremental compilation disabled for that invocation.

The child-tree control suite has13 passing tests, including3 new regression
workflows. A poisoned real child-store lock proves that concurrent session
shutdown, repeated shutdown and host removal preserve cleanup failure. A held
active child proves that simultaneous host shutdown calls both wait, caller
cancellation leaves cleanup running, and a successfully drained host can admit
a new generation. Fake backends and explicit temporary state roots keep these
checks independent of provider credentials and ambient memory services.

```bash
cargo fmt --all
cargo test -p rara-app-server --lib --locked --offline
cargo clippy -p rara-app-server --all-targets --locked --offline -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked --offline --test runtime_session
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib tools::agent::agent_control::tests
cargo clippy --locked --all-targets --no-deps --offline -- -D warnings
git diff --check
```

These checks cover the codec and canonical session/cursor behavior. The remaining
runtime command seam, process transport, remote CI and child-process smoke
evidence remain open until their implementations exist.

## Follow-Ups

Implement and validate the canonical session command seam, bounded process
transport, receipt/replay behavior and real isolated child-process smoke before
advertising the protocol. The owning contract is
[App Server Stdio Protocol](../features/app-server-stdio.md).
