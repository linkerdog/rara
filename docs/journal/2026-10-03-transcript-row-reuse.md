# Shared Transcript Rows

## Scope And References

This continues #921 after source-cache head
`945d0f42f9fa046273a707546ce15150056cc4f8`. The target is to remove complete-history
clone/wrap/hash work from frames and scroll navigation, using the same rows for
rendering, highlighting, and copied text. Active-cell assembly is a separate
remaining cost; no database, protocol, dependency, or terminal policy changes
are included.

Inspected references before implementation:

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`, `pager_overlay.rs`: immutable
  history cells and cached heights, append preserving old cells, and live-tail
  invalidation including width, revision, continuation spacing, and animation.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`, `Message.tsx`: static
  message identity with width/visibility keys; `OffscreenFreeze.tsx` reuses
  references but deliberately avoids freezing virtual-list content.

Adaptation: immutable shared row blocks with cumulative row boundaries, a
replaceable active block, and selection consuming the same snapshot. Render only
the visible window. Do not freeze offscreen content or infer identity from
equal edge rows. Complete styled-line equality conservatively guards the active
layout cache because presentation state does not yet have exhaustive revisions.

## Long Plan And Preparation

1. Instrument real clone/wrap/hash/text-construction boundaries per instance on
   the frozen parent. Add production-buffer work guards for repeated frames,
   scroll/copy, and streamed deltas. Exit with behavioral work-count REDs.
2. Build indexed shared history/active visual rows. Reuse unchanged history,
   append only newly committed blocks, and publish the same immutable rows to
   selection. Exit with those guards passing and unchanged rendering/copy.
3. Validate same-size middle changes, styling, width/height, thinking visibility,
   finalization/replacement, thread/reset, and long history. Update stable specs
   and active TODO, then deliver normal commits/stacked PR and exact-head CI.

The selected approach avoids full-history fingerprints. Retrofitting a revision
across every active presentation mutation would be broader and risks stale
output if one write or animation is missed. Active-cell assembly and its exact
comparison cost remain explicit, not counted as historical-row reuse.

Local source/docs edits, focused Cargo checks, default-config Bazel, and normal
branch/push/PR delivery are authorized workflow steps. No new permissions or
schema choices are required.

## Validation

The frozen parent's production work boundaries produced four valid behavioral
REDs on a 1,000-line committed fence. The following counts are deltas after an
initial frame, not total process allocations:

| Scenario | Parent clone / wrap / text / hash rows | Shared rows clone / wrap / text / hash rows |
| --- | --- | --- |
| 20 unchanged frames | 20,320 / 20,040 / 0 / 20,040 | 280 / 0 / 0 / 0 |
| 20 scroll inputs and frames | 40,380 / 40,080 / 20,040 / 20,040 | 300 / 0 / 0 / 0 |
| 20 selected copy frames | 20,320 / 20,040 / 0 / 20,040 | 280 / 0 / 0 / 0 |
| 50 small streamed deltas and frames | 50,800 / 50,350 / 50,407 / 50,407 | 700 / 250 / 307 / 0 |

The meters are per-instance and test-only. Clone counts cover the former
committed-cache clone and the visible window; wrap counts cover logical lines
sent through the visual-row boundary; text counts cover snapshot row text
construction. The production full-history hashing path is removed. Active-cell
assembly, styled-tail comparison, sanitizer/control-token processing, parser
work, and block-index metadata copies are not included. These counts must not
be read as a bound on all work in a frame or on arbitrary Markdown.

The first thinking fixture contained only two lines, both legitimately visible
in the collapsed preview. Its failed assertion was a test bug, not a fifth
baseline product RED. The corrected fixture puts the hidden detail on line
three. A controlled mutation omitting thinking visibility from the history key
reproduces stale expanded content and fails that production-buffer guard; the
real key is restored before the final checks. Initial compilation errors and
an unprefixed test-row assumption are likewise excluded from behavioral RED
evidence.

Focused guards also cover retained styled/text allocations over 20 committed
appends; complete middle text, line/span style, and alignment invalidation;
unchanged-tail allocation reuse; width reflow and height-only reuse; cwd and
thinking visibility; active and late committed replacement; reset/restore;
segmented indexing with blank/empty blocks and cross-block copy. The existing
70,000-row production render/highlight/copy regression remains unchanged, as do
the Unicode width matrix and snapshots.

Check commands:

```bash
cargo test --locked --lib transcript_cache -- --nocapture
cargo test --locked --lib tui:: -- --nocapture
cargo test --locked --lib
cargo check --locked
cargo clippy --locked --all-targets -- -D warnings
cargo fmt --all -- --check
git diff --check
bazel test //:rara_unit_tests --test_output=errors
```

The 12 new focused guards pass within the 741-test TUI suite. Root Cargo and
default-config Bazel both report 1,606 passing tests and one ignored paid-call
fixture. Compilation, strict all-target Clippy, formatting, and diff checks
pass. Bazel invocation: `63da7b3e-18f7-4bc0-8045-ca62eb83d6c5`. No snapshots were
regenerated. Touched Rust sources remain below 1,000 lines, and `mod.rs` remains
a facade. The existing macOS debug-linker compact-unwind warning is unchanged;
it is not a new Rust compiler or Clippy warning.

## Follow-Ups

- Active-cell assembly and long mutable/source-wide Markdown replay still need
  explicit work boundaries; the full #921 issue remains open.
- Active-tail equality and changed-block rewrapping still scale with active
  content. Appending with retained snapshots copies block-index metadata;
  many-short-turn costs remain separate from historical row-content reuse.
- Stateful sanitization and progress retention remain separate #923 work.
- Remote CI, review, merge, and real-terminal acceptance are separate gates.

## Review Integration

Merged the updated #939 parent, `409b57a148206e4cdd5e16b5d229bde429f3c009`,
without rewriting the branch. This includes current main, the recovered scroll
change, timed-paste bookkeeping, and the plain-fence blank-span correction.
Cargo inputs and generated dependency state remain the parent's versions.

The review identified missing theme invalidation in both committed rows and
the live Markdown cache. Before implementation, rechecked Codex's
`pager_overlay.rs` live-tail key (width plus presentation revision) and Claude
Code's `Markdown.tsx`: tokens are cached independently of styling, while the
render memo includes the active theme. The adaptation retains existing caches
and adds semantic/syntax revision keys, rebuilding styled source rows on the
next presentation read even if no new delta arrives. Installing the same
resolved theme does not increment either revision. This fixes cache behavior
without introducing a theme-switch command or a new runtime handle.

Two isolated child-process tests reproduced stale heading colors in committed
and streamed rows before the fix. They then check both semantic and syntax
changes against fresh canonical rendering, and check reuse after reinstalling
the same theme. Child processes prevent global palette mutation from racing
other app fixtures. A deterministic mixed-mutation guard compares all styled
rows with fresh wrapping through 480 append, commit, replacement, visibility,
cwd, restore, reset, stream, and width transitions while retaining old snapshots.

Documented the UI-task ownership behind `Rc`. The retained block-handle copy
cost is still explicit; its persistent-index follow-up is in #941 and is not
silently counted as constant-time work here.

Review validation: `cargo test --offline --locked --lib transcript_ -- --nocapture`
passes 110 tests. The full TUI filter passes 781 tests with one intentionally
ignored terminal child fixture. No snapshots changed; touched Rust sources stay
below 1,000 lines. Formatting, strict Clippy, and exact-head remote CI remain
required delivery checks.
