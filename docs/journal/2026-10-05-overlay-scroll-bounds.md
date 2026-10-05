# Bounded Read-Only Overlay Scrolling

## Background

Issue #980 identifies inaccessible Status content, clipped Help sections, and
unbounded Context offsets. The contract is INPUT-06 in the interaction spec.
This work changes TUI presentation and input ownership only; runtime snapshots,
control-plane requests, persistent data, and provider behavior stay unchanged.

## References And Plan

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
  `codex-rs/tui/src/pager_overlay.rs`: width-aware content measurement,
  viewport-sized navigation, render-time clamping, and a fixed header/footer.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
  `src/ink/components/ScrollBox.tsx`, `src/ink/render-node-to-output.ts`,
  `src/components/ScrollKeybindingHandler.tsx`, and
  `src/components/HelpV2/HelpV2.tsx`: separate body/viewport dimensions, bounded
  renderer offsets, viewport-derived page actions, and a separate command-list
  selection contract.
- Adapt these patterns using the existing styled row wrapper. Materialize the
  same rows for viewport bounds and output; keep numeric state free of terminal
  widgets. Use one continuous Help Runtime body and wrapped command rows.

## Stages And Acceptance

1. Numeric scroll state: isolate offsets, measured geometry, page/boundary
   navigation, and selection reveal. Exit on unit coverage for extreme deltas,
   resize/content shrink, empty geometry, and offsets beyond 65,535 rows.
2. Projection and routing: integrate Help/Status/Context bodies and their keys
   and wheel; preserve command-entry selection, composer ownership, and fixed
   chrome. Exit when final rows are reachable and no input accrues scroll debt.
3. Acceptance: exercise all tabs at 80x24, 60x20, and 40x12, with long lines/CJK,
   runtime content changes, resizing, and switching tabs. Check existing TUI
   contracts, formatting, and strict Clippy before publishing the PR.

The key paths are feasible with the current `wrap_lines` output and mutable
render projection. Local file edits, Rust checks, feature-branch publication,
and PR creation are already authorized. Default remote Bazel remains the
acceptance gate; no build configuration or cache workaround is included.

## Trade-offs

Help Runtime uses vertical reading order at every width instead of fixed
columns. Manual scrolling retains numeric row anchors across reflow rather
than attempting semantic text anchoring. The model stores only current overlay
geometry; opening an overlay or changing tabs resets the view. The unmeasured
body follows the existing transcript policy of ignoring row-scroll input until
the first frame.

## Implementation

- `OverlayScroll` owns only numeric geometry and navigation. Rendering and
  input share current wrapped rows and clamp offsets without accumulating
  overflow. Pages overlap by one visual row; Home/End reach both boundaries.
- The read-only overlay renderer is split out of `overlay.rs`. A common body
  keeps tabs and hints fixed, preserves styles through shared wrapping, and
  slices with `usize` offsets instead of narrowing through paragraph scrolling.
- Help Runtime retains every existing section in one continuous body. Help
  Commands reuses the palette's entry projection, wraps descriptions, and
  reveals selected entries. Paging and boundary navigation can expose the last
  row of a wrapped description; reopening the tab resets its selection.
- Keyboard and wheel input route to the current overlay. The composer and
  transcript retain their state. Content changes refresh bounds before input;
  resizing refreshes dimensions and clamps the row anchor during rendering.
- The completed Help scrolling TODO is removed; resume editing, paste
  placeholders, Vim mode, and physical terminal acceptance remain separate.

## Validation

- The isolated Rust scroll model passes five tests for extreme deltas, page
  size, empty/unmeasured geometry, shrink/resize, entry reveal, and large rows.
- Seven renderer/harness tests cover 21 tab/terminal-size combinations, every
  final visual row, complete CJK paths and Runtime help sections, command entry
  selection, repeated boundary input, wheel routing, resize/content shrink,
  tab reset, and draft/transcript isolation. Tiny geometry is also checked.
- `cargo test --locked --lib tui:: -- --nocapture`: 1,057 passed, four existing
  ignored fixtures. No test depends on sleeps or a physical clipboard.
- `cargo clippy --locked --all-targets --no-deps -- -D warnings`: passed.
- `cargo fmt --all -- --check` and `git diff --check`: passed.
