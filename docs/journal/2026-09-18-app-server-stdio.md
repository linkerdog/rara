# App Server Stdio Protocol Foundation

## Summary

Define the version1 server-first stdio boundary needed by an external process
supervisor. The shared codec, explicit cursor subscription, retained shutdown
outcomes, targeted turn stop commands, bounded prompt source commands and strict
native input/approval commands are implemented. The wire control frame carries the
expected turn for stops and answers. This does not yet make the app-server command available or complete
a downstream provider integration.

The native skill catalogue now resolves bounded inline protocol definitions and
the native skill tool can list/invoke them. Canonical registration ownership and
model-visible discovery metadata are still follow-up work; the old protocol
registry has not yet been connected to this catalogue.

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
- Fence cancel/interrupt to the expected turn. Keep their distinct terminal
  outcomes and retain the first accepted stop kind while execution drains.
  Repeat requests do not relabel that kind; shutdown preserves an accepted
  interruption. A receipt acknowledges the stop request, not provider completion.
- Route bounded prompt source registration through the idle session actor and
  existing per-query context assembly. Preserve lifecycle provenance and reject
  unsupported scope/layer/persistence claims. Extract the prompt registry from
  the near-limit protocol sources module. Skill bodies still require the owning
  SkillManager/SkillTool path and are not appended indiscriminately to prompts.
- Own pending interaction identity in the session actor. Strict answers name the
  originating turn and native response kind; new prompts cannot replace a wait.
  Snapshots and replay carry the same descriptor before the terminal turn event.
  Busy follow-up rejects explicitly. Stops/close discard callbacks; legacy plain
  submission records replacement. No persistent approval lifetime is claimed.
- Give native plan/shell continuations their own inference lease and query report.
  The old direct continuation path could retain the preceding query's report and
  accounting context. Refresh sources only for continuations that need a model
  call, attach them to new continuation context, and emit plan rejection settlement.
- Require session and expected-turn targets for stop and answer wire methods.
  Other methods cannot silently ignore a supplied target. The codec validates
  shape and bounded identity; the actor validates current ownership. Receipt
  equality will include the target when the transport is implemented.
- Resolve inline skills through the native catalogue, below local definitions.
  Protocol priority then original registration order chooses a winner; disabled
  and shadowed definitions retain metadata. Enforce count/body/aggregate limits
  before replacement. Native tool listing stays body-free, invocation returns
  instructions with source identity, and local reload preserves protocol records.
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

The targeted stop follow-up brings the runtime-session integration suite to8
passing tests. Its held-provider workflow checks stale turn rejection, repeated
and conflicting stop requests, acceptance before execution completion, distinct
cancellation/interruption events, partial evidence, and shutdown preserving a
previously accepted interruption. Neither a queue nor approval persistence is
claimed by these commands.

Prompt source validation has11 passing registry tests, including4 new cases for
invalid/unsupported registration, count/aggregate capacity, atomic replacement,
and lifecycle provenance. All5 existing context-view tests pass. One new isolated
runtime-source workflow proves actual backend request delivery,
stable system prefix, cross-session rejection/isolation, busy mutation rejection,
query-count expiry and rejection after close. The source limits are explicit;
the current user-context renderer does not pretend to apply system/developer
layers or persist sources across process exit. The final session workflow lives
in the library test target so both Cargo and the existing Bazel source glob
discover it without a build configuration change.

Four new input workflow tests cover successive questions, wrong-kind/stale/duplicate
answers, busy follow-up, blocked prompt/transcript replacement, immutable event
replay, stop/close and legacy discard, all three native plan decisions, shell
approval/denial, provider tool identity, fresh accounting and continuation context.
The fixtures set the native mode before actor ownership and use explicit isolated
state, fake providers and a shell recorder that never launches commands. All47
existing planning tests and8 runtime-session integration tests pass. Plan rejection
performs no model request, preserves one-query source eligibility and reports no
stale usage. Native question parsing remains limited to plan mode.

The wire targeting follow-up brings the shared protocol suite to19 passing tests.
Three new table-driven cases cover all fenced method variants, absent session or
turn targets, null turn targets, misplaced targets, inclusive ID bounds and safe error
messages. Existing prompt, replay, shutdown and handshake golden shapes remain
unchanged. Generic approval frame validity does not advertise that method as
implemented; runtime capability negotiation remains authoritative.

The native skill crate has 9 passing tests, including 5 new cases for deterministic
resolution, local authority, atomic invalid replacements, count/byte bounds,
disabled metadata, reload and explicit body disclosure. All 8 native skill-tool
tests pass, including a new list/invoke/disable workflow with source identity and
body-free disabled status. The skill crate passes all-target Clippy with warnings
denied. These tests establish the catalogue/tool boundary, not live protocol
registration or automatic model-context delivery.

```bash
cargo fmt --all
cargo test -p rara-app-server --lib --locked --offline
cargo clippy -p rara-app-server --all-targets --locked --offline -- -D warnings
cargo test -p rara-skills --locked --offline
cargo clippy -p rara-skills --all-targets --locked --offline -- -D warnings
CARGO_INCREMENTAL=0 cargo test --locked --offline --test runtime_session
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib tools::agent::agent_control::tests
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib protocol_sources::
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib agent::tests::context_view::
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib runtime_session::source_tests::
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib runtime_session::input_tests::
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib agent::tests::planning
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib tools::skill
cargo clippy --locked --all-targets --no-deps --offline -- -D warnings
git diff --check
```

These checks cover the codec and canonical session/cursor behavior. The remaining
runtime command seam, process transport, remote CI and child-process smoke
evidence remain open until their implementations exist.

## Follow-Ups

Bind canonical skill registration to the native catalogue/tool manager, deliver
compact metadata to model context with stable system guidance, and emit real
invocation provenance. Keep unsupported root discovery explicit. Implement bounded process transport,
receipt/replay behavior and real isolated child-process smoke before advertising
the protocol. The owning contract is
[App Server Stdio Protocol](../features/app-server-stdio.md).
