# App Server Stdio Protocol Foundation

## Summary

Define the version1 server-first stdio boundary needed by an external process
supervisor. The shared codec and its focused checks are implemented. This does
not yet make the app-server command available or complete a downstream provider
integration.

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

```bash
cargo fmt --package rara-app-server
cargo test -p rara-app-server --lib --locked --offline
cargo clippy -p rara-app-server --all-targets --locked --offline -- -D warnings
git diff --check
```

These checks cover the codec only. Runtime, process, remote CI and live smoke
evidence remain open until their implementations exist.

## Follow-Ups

Implement and validate the canonical session command seam, bounded process
transport, receipt/replay behavior and real isolated child-process smoke before
advertising the protocol. The owning contract is
[App Server Stdio Protocol](../features/app-server-stdio.md).
