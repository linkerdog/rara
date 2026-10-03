# Stack Review Integration

## Scope

Read all comments, reviews and review threads for #936–#944 after the new
review batch. The mainline conflict is a delivery blocker even though the old
heads had green CI. Preserve original commits with normal merge-forward, never
rebase or force-push. The unpublished #925 viewport checkpoint is separately
preserved at `dbb48c12`; it is not part of this integration lane.

Frozen mainline: `c036008e8b7b19d6887a34870b7ac3e08047528b`, containing
#932 terminal panic/mode ownership, #933 paste ordering, #934 durable runtime
goals and #935 composer geometry.

## Long Plan

1. Integrate main into #936. Inputs are the frozen mainline and original head
   `16030cafbf859744a6bf9bbc4adb5675cbe6d985`. Preserve both mainline behavior
   and shared grapheme wrapping. Prefer merge-forward to rebuilding commits;
   its cost is visible merge commits, not rewritten review history. Exit after
   Cargo/Bazel/lint proof, generated-lock audit and exact pushed-head readback.
2. Propagate each qualified parent into the next branch, #937 through #944.
   Resolve semantic conflicts rather than selecting an entire source side.
   Retain GoalStore restore validation and panic-owner guarantees. Enter with
   the parent qualified; exit with each child's focused checks and exact-head
   CI. Old evidence remains tied to the old head.
3. Address remaining findings in their owning PRs. In particular, verify
   #942 stop-result arbitration, compact-event fencing and completion-stream
   loss; bound #943 unterminated control strings; reproduce #939's plain-fence
   blank-row style mismatch. Add genuine behavioral failures before fixes,
   align the owning specifications and reply with source/evidence boundaries.
4. Reconcile #925 against the integrated tip, preserving both scoped output
   and mainline panic handling. Complete key/job-control acceptance separately.

Prepare uses existing checkout branches, default Cargo/Bazel, generated lock
updates and scoped GitHub branch/comment writes. No manual BUILD/configuration,
database or persisted-format change is planned. The unrelated #888 worktree
still awaits its separate BUILD authorization.

## Root Conflict Decisions

- The composer add/add is squash ancestry, not competing new designs. Keep
  #936's shared grapheme-range implementation and all geometry tests; main's
  #935 scalar implementation predates it.
- Keep main's paste test registration and production paste-flush/edit/submit
  ordering, alongside shared wrapping documentation.
- Mode guards, GoalStore restoration and their tests match the frozen mainline
  byte-for-byte at the root integration boundary.
- Select the mainline generated lock as a valid regeneration input, then let
  default Bazel regenerate it from the merged Cargo graph. Never hand-merge
  embedded crate-universe contents or source hashes.

## Validation

Root integrated-source results:

- `cargo test --quiet --lib --tests`: 1,593 root tests and nine integration
  tests passed. The paid-provider experiment and explicit terminal panic child
  fixture remain ignored; parent tests invoke the isolated terminal fixture.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed.
- `cargo fmt --all --check` and `git diff --check`: passed.
- Default `bazel test //:rara_unit_tests --test_output=errors`: executed and
  passed, invocation `67bf5e8c-698d-4be7-9a7d-0ef9a1f07de4`.
- Structural lock comparison against frozen main found only Cargo-input hashes,
  generated root dependencies and unicode-segmentation aliases; no version
  upgrade or manual BUILD/configuration change.

These are local integrated-source results, not new-head remote CI, stack-wide
qualification, merge, or native terminal acceptance. The known macOS compact
unwind linker warning also occurs on the parent.

## Follow-Ups

Finish the phased lane above. No issue is closed and no merge or production
acceptance is claimed by this checkpoint.
