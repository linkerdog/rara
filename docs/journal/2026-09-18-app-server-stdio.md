# App Server Stdio Runtime Control

## Summary

Define the version1 server-first stdio boundary needed by an external process
supervisor. The shared codec, explicit cursor subscription, retained shutdown
outcomes, targeted turn stop commands, bounded prompt source commands and strict
native input/approval commands are implemented. The wire control frame carries the
expected turn for stops and answers. The exact version1 stdio CLI now runs through
canonical session/host ownership. This does not complete downstream integration.

The native skill catalogue resolves bounded inline protocol definitions through
canonical session registration and the real SkillTool. Compact metadata reaches
model-visible context; invocation records source and turn provenance. Static
system guidance stays stable across catalogue changes. Process transport advertises
only the implemented method set and has isolated real-child evidence.

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
- Bind inline skill commands and the native tool to one session catalogue.
  Reject active turns, foreign provenance, unowned host tools, restricted profiles
  and unsupported root discovery. Local reload uses the explicit workspace and
  preserves protocol definitions; disabled ambient discovery cannot be reopened
  by tool reload. Replace synthetic injection events with native invocation evidence.
- Persist compact skill metadata on the latest model-visible user/continuation
  context, including a single clear marker after removal. Preserve historical
  bytes and static system guidance. Context inspection no longer reports available
  metadata as injected before it reaches model history.
- Mirror connection gating and bounded ordered output from the inspected Codex
  transport, and correlated/cancelled control responses from the inspected Claude
  Code transport, without adopting either wire schema or implementation.

Reference revisions: Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24` and the local
Claude Code checkout `4b9d30f7953273e567a18eb819f4eddd45fcc877`. These are pinned
source observations, not claims about current released versions.

## Process Transport Checkpoint

The exact CLI flags now select a bounded stdio adapter. It flushes the server-first
handshake, normalizes client source provenance, dispatches through canonical
sessions and forwards their original event identities. State query and finite
replay have owned session API seams. Unsupported methods remain explicit rejects.

A bounded receipt table fingerprints canonical request JSON with the existing
SHA-256 dependency and retains pending/resolved identity without eviction. A
reserved slot permits shutdown after ordinary request capacity is exhausted.
Duplicates repeat the ACK; conflicting content cannot reapply an operation.

Dedicated standard I/O threads keep blocking stdin outside Tokio teardown. One
writer preserves frame order, with bounded queues and write/drain deadlines. EOF
before semantic shutdown,
framing failure and output loss drain owned sessions without claiming semantic
shutdown. Successful shutdown drains event forwarding before the completion frame
and exits even when the supervisor leaves stdin open. The native permission bypass
still requires an explicit startup flag; the transport does not accept claimed
client trust as runtime authority.

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
denied. The following binding checkpoint additionally establishes canonical
registration and model-context delivery.

The binding checkpoint has 26 passing focused root tests: session source/input
workflows, native skill tools, context inspection, typed context and cache-prefix
regressions. Three new canonical workflows prove actual native list/invoke tool
results, source/turn events, no premature body disclosure, disable, stable system
prefixes, append-only history, cross-session isolation, busy rejection, backend
replacement and unsupported tool/profile/root cases. A new context-view regression
separates available from persisted metadata. Local reload coverage verifies the
explicit workspace and retained protocol records. All 35 instruction-crate tests
and 9 skill-crate tests pass, including a new empty/nonempty catalogue prefix
regression. Root all-target Clippy passes with warnings denied.

The final focused root selection has35 passing tests. Nine new transport/CLI
tests pass and cover: retained duplicate/conflict receipts, capacity
and the shutdown reservation, pending uncertainty, turn-target fingerprints,
LF/CRLF and inclusive frame limits, actual session dispatch, canonical replay/gaps,
source provenance, stale runtime/session/turn rejection, cancellation, duplicate
receipts during shutdown drain, rejection of new closing work and owned
cleanup after input/output failure. The real binary passes five isolated process
scenarios through `scripts/app_server_smoke.py`: one fake-provider request and
semantic shutdown with stdin still open; EOF; malformed input; truncated input;
and stdout loss. No personal credentials, memory services or paid provider are
used. The debug smoke binary alone was linked without debug information to fit
available disk space; project build configuration and dependencies are unchanged.
The test workflow also runs this process smoke against its built binary so that
open-stdin shutdown and transport-failure behavior remain CI regression gates.

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
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib -- runtime_session:: tools::skill agent::tests::context_view agent::tests::prompt_cache model_context::
cargo test --locked --offline -p rara-instructions -p rara-skills --lib
CARGO_INCREMENTAL=0 cargo test --locked --offline --lib -- app_server_stdio:: app_cli::tests::app_server_requires
CARGO_INCREMENTAL=0 cargo rustc --locked --offline --bin rara -- -C strip=debuginfo
python3 scripts/app_server_smoke.py target/debug/rara
cargo clippy --locked --all-targets --no-deps --offline -- -D warnings
git diff --check
```

These checks cover the codec, canonical session behavior and actual process
transport. Remote CI and downstream supervisor integration remain separate gates.

## Follow-Ups

Keep unsupported root discovery, durable resume and additional control families
explicit until their own ownership and recovery contracts are implemented. The owning contract is
[App Server Stdio Protocol](../features/app-server-stdio.md).
