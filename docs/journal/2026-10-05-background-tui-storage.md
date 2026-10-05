# Background TUI Storage

## Scope And Reference Review

Issue #978 covers blocking runtime checkpoints, live transcript appends, turn
commits, resume listing/loading, and shared-task scans on the event loop. The
separate `/review` capture fix is delivered by PR #1027; retain that dependency
when auditing the issue rather than recreating its implementation here.

Reviewed Codex at `f959e7fc`: `rollout/src/recorder.rs` serializes commands with
explicit persist/flush/shutdown acknowledgements and retains failed writes;
`tui/src/resume_picker.rs` loads pages/transcripts off the input loop and fences
results with request/search tokens. Reviewed Claude Code at `4b9d30f7`:
`utils/sessionStorage.ts` batches per-file writes on a 100 ms timer and drains
pending writes during cleanup. Adapt ordered ownership, batching, and explicit
completion; keep the existing file/SQLite contracts.

Nowledge recall for background persistence and restore was attempted but the
server requires `exact-v1` Space support. Credentials and Space were unchanged.
The current implementation and this checkpoint remain the available evidence.

## Long Plan

1. Ordered writer and durability: capture owned immutable write requests;
   coalesce runtime checkpoints and batch live entries; preserve failed writes
   ahead of later clears; add read/flush acknowledgements. Exit when an injected
   blocked store proves nonblocking submission, order, and retry semantics.
2. TUI persistence integration: replace direct I/O with writer admission and
   project completion/errors without recursive writes. Distinguish displayed
   turns from durable acknowledgement; drain before exit and session replacement.
   Exit when live-log, turn, failure, and shutdown regressions pass.
3. Background reads: debounce/fence resume listing, prepare selected threads
   without consuming/mutating the active agent, and scan shared task files off
   the tick. Exit when stale-result, failure-isolation, and pending-input tests
   pass, including the 10,000-entry restore fixture.
4. Integration/acceptance: run focused tests, the TUI suite, strict workspace
   Clippy, formatting, and default remote CI. Review the complete diff against
   every #978 call path before using a closing reference in a PR.

Independent blocking tasks per write would race live-log clearing against turn
commits. Use one ordered owner instead; read barriers preserve read-after-write
semantics. Ordinary workspace edits, local tests, and feature-branch publication
are already authorized. No database migration or history rewrite is required.

## Implementation Checkpoint

The storage owner isolates read/write panics so later accepted writes keep their
owner; failed writes retain their operation for retry. It accepts immutable requests, batches adjacent live fragments,
coalesces adjacent checkpoints, and serializes writes with read/flush/shutdown
barriers. Failed operations remain at the head of the queue; later live-log
clears cannot discard recovery data. Appends preserve a newline boundary after
an interrupted JSON fragment so a successful retry remains independently readable. Error projection does not recursively
persist its own failure notice. Accepted writes remain lossless in memory;
full transcript/admission memory bounds remain the separate #1008 contract.

Resume searches debounce for 150 ms and fence old results. Selected threads are
materialized on the storage owner, including migrations, todo/runtime/goal reads,
10,000-entry transcript construction, and live recovery entries. The current
agent stays owned by the runtime throughout preparation. Failure and cancellation
preserve it. Applying a successful result clears old session-local interactions
without deleting live data. Draft submission waits; deferred permissions apply
after the outcome, and startup plugin rebuilding waits for the restored binding.
Provisional snapshots do not enqueue checkpoints while a restore is pending,
preventing an unused fresh session from becoming the next latest-thread target.

The event loop waits for an explicit write acknowledgement on quit and keeps
rendering. Esc cancels the exit but retains accepted writes. Save failure leaves
the session open. Startup/terminal error cleanup also drains and joins storage;
a secondary cleanup error is logged even if the primary failure is returned.

## Filesystem Context Boundary

The audit found an indirect read path: `apply_runtime_snapshot` previously called
`Agent::shared_runtime_context`, reading workspace prompt/memory files, Git HEAD
metadata, and shared tasks. Display assembly now consumes owned filesystem inputs
from one background refresh, with session/config/mode generation fencing. Shared
task scans have a separate single in-flight job and binding fence. Context views
show initial loading state; unchanged file results do not rebuild snapshots or
enqueue duplicate checkpoints. Filesystem inputs refresh every two seconds and
shared tasks every 500 ms. Actual model-turn assembly stays fresh and retains
prompt ordering, budget calculation, provider cache behavior, and history.

The touched context assembler `mod.rs` is now a facade; its implementation and
existing tests live in `assembly.rs`. No persistence format or protocol migration
is introduced. The separate `/review` fix landed in PR #1027. Main at `74ae3cd4` also
contains #1024 event recovery and #1026 panic lints; it was merged into this
branch. The only conflict was the runtime command admission guard, resolved by
retaining both review-preparation gating and restore cancellation for a new
runtime request. Post-merge validation is recorded below.

## Validation

Focused regression coverage includes blocked-store key dispatch/rendering,
coalescing/barrier order, failed canonical commits preserving live logs, retry,
cancellable quit with a blocked store, stale search and context replies, stale
shared-task scans, a 10,000-entry restore with responsive input and cancellation,
permission changes across restore success/failure, startup rebuild ordering, and
session-local interaction replacement without live-log deletion. Existing
persistence tests now wait for explicit acknowledgements rather than wall-clock
sleeps. Fresh display projection is compared with model-context assembly.

Local validation:

- `cargo test --lib tui:: -- --nocapture`: 1,056 passed, four existing ignored after merging current main.
- `cargo test --lib context:: -- --nocapture`: 67 passed.
- `cargo test --lib thread_io:: -- --nocapture`: five passed, including read/write panic isolation.
- `cargo test --lib runtime_goals -- --nocapture`: 11 passed.
- `cargo test -p rara-persistence`: nine passed, including append-boundary recovery.
- `cargo clippy --workspace --all-targets -- -D warnings`: passed.
- `cargo fmt --all --check` and `git diff --check`: passed.

Remote CI remains the merge acceptance gate.
Default local Bazel analysis remains unavailable because the existing external
cache lacks `rules_rust//rust`; its configuration and cache were not changed.
