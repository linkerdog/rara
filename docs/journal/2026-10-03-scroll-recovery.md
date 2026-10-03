# Transcript Scroll Recovery

## Background

PR #937 is marked merged, but its scroll changes are absent from `main` at
`cb4f58a8`. The ancestry repair described in the stack integration journal
preserved the old commit while restoring the parent tree. GitHub's merge status
therefore does not establish delivery of issue #920.

## Implementation

Reapply only the delta from `8e727881` on the current mainline in a new branch.
Preserve the terminal owner, paste ordering, durable GoalStore, and shared
wrapping already on main. The runtime-feedback documentation keeps both the
scrolling contract and the terminal-restoration contract.

The original numeric `FollowTail` / `Anchored(top)` state, refreshed input-time
measurements, `usize` viewport/selection offsets, and focused regressions are
retained. Existing oversized state tests are split by responsibility without
dropping tests. The original journal's test results describe the old head only.

## Reference And Review Decisions

The local Codex pager clamps against measured rows and preserves explicit
bottom-following through insertion. Claude Code's `ScrollBox` separates manual
navigation from sticky bottom mode and provides fresh measurements. The
recovery retains this separation rather than inferring user intent from a
temporary layout clamp.

The four non-blocking observations on #937 remain explicit:

- Anchors identify visual rows, not semantic content across reflow.
- Shrinking content clamps an anchor without enabling follow mode. The state
  regression covers shrink followed by growth; explicit navigation to the end
  or starting input enables following.
- Rebuilding rows per scroll event is owned by the subsequent #921 cache work.
- Page navigation retains the existing eight-row step.

## Validation

Validate the recovered head independently of the old PR's CI:

```bash
cargo test --locked --lib transcript_scroll -- --nocapture
cargo test --locked --lib tui::state::tests
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
```

These checks cover the recovered source and production renderer, not physical
terminal acceptance. Remote CI must run on the new PR head. No Cargo manifest,
Cargo lock, or generated Bazel lock change is required by this recovery.

## Follow-Ups

Propagate the recovered branch into #938 and then through #944 with ordinary
merges. Each PR needs review fixes and validation against its integrated parent.
Issue #921 and terminal lifecycle acceptance remain separate work.
