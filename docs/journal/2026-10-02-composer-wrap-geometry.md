# Composer Wrap Geometry

## Summary

Issue [#919](https://github.com/linkerdog/rara/issues/919) identified an
indent-blind composer wrap cache and vertical cursor movement using terminal
width instead of the rendered main-pane width. Both could make the cursor point
at a different character from the selected input offset.

## References And Plan

- Codex `tui/src/bottom_pane/textarea.rs` shares cached wrapped lines among
  height, cursor placement, rendering, and vertical navigation. It navigates
  displayed rows rather than independently wrapping the terminal width.
- Claude Code `src/hooks/useTextInput.ts` builds `Cursor` with the input
  columns and prefers wrapped-line navigation. `src/utils/Cursor.ts` shares
  measured rows and position-to-offset conversion with display.
- Adapt these patterns to a private plain-text layout. Keep the existing
  character-offset editor and fixed four-column tab behavior. Share pane
  geometry instead of giving state a dependency on a renderer.
- Review follow-up: Codex's `move_cursor_up` chooses a target column against
  a wrapped text range; Claude Code's `Cursor.up/down` converts display columns
  back into offsets through measured rows. Preserve distinct insertion
  boundaries before hardware clipping. Setup fields are intentionally clipped
  single-line editors, so they must not inherit the composer's soft wrapping.

## Implementation And Trade-offs

- `composer_text.rs` owns wrapped rows and numeric cursor positions without
  Ratatui lines, spans, styles, or layout objects. Its cache key includes text,
  width, and both indents. Matching callers share an immutable layout.
- Rendering, height, scroll, cursor placement, and vertical movement use the
  same composer configuration. Continuations have a two-column indent rather
  than height measurement silently dropping that indent.
  Placeholder rows use this same layout and discard stale draft scroll.
- `pane_geometry.rs` centralizes the existing sidebar threshold and width.
  Renderer and navigation compute main-pane width from the same geometry.
- At a soft boundary before another character, cursor membership follows that
  character to the next row. At the full final row, the cursor retains the
  existing last-visible-column behavior. Vertical movement chooses the
  closest display column, retaining the existing character-offset contract.
- Remove the duplicated state wrapping and display-width implementations.
  Transcript-wide wrapping, grapheme editing, and terminal lifecycle are not
  included in this fix.
- Review follow-up separates logical insertion columns from clipped hardware
  cursor columns. Offset lookup compares the stored columns, not their clipped
  aliases, so the last character does not tie with a newline or end boundary.
- URL, model-name, profile-label, and API-key editors retain their unwrapped
  rendering and clip the cursor on that same row. Shared tab expansion is used
  by both plaintext editor rendering and its cursor measurement. Masked keys
  use the actual asterisks for cursor measurement, not the secret's Unicode
  widths. No horizontal-scroll or grapheme-editing API is added.

## Validation

Behavioral RED: requesting composer indents after no-indent rows returns
`["abcdef", "ghij"]` instead of `["› abcd", "  efgh", "  ij"]`. A rendered
cursor regression also reads `j` at the cursor cell when the selected input
offset points to `h`.

Review REDs: at 80 columns, offset 155 in a 156-character draft becomes 156
after Up/Down, changing insertion to append. In the production URL editor,
offset 76 returns `(2, 9)` on an empty row instead of `(77, 8)` on the clipped
text row. Corrected tests cover the last character before end-of-input and
newlines, distinct full-row insertion boundaries, actual insertion, all four
setup surfaces, soft-boundary/overflow offsets, and masked wide-character keys.

Focused coverage includes indent isolation, height measurement, actual screen
buffer cells, Up/Down round trips at 80/120/160 columns with the sidebar on/off,
soft-boundary ownership, blank lines, tabs, wide characters, and cache changes
after input or width changes.

```bash
cargo test --locked --lib bottom_pane_tests -- --nocapture
cargo test --locked --lib composer_text -- --nocapture
cargo test --locked --lib tui:: --quiet
cargo check --locked
cargo clippy --locked --all-targets --no-deps -- -D warnings
cargo fmt --all -- --check
git diff --check
```

Remote exact-head CI and manual terminal interaction remain separate from
these pure layout and production-render checks.

The pre-review TUI-filtered suite reports 659 passing tests. `cargo check`, strict
all-target Clippy, formatting, and diff checks complete without source
warnings. The macOS debug test linker retains its existing large `__eh_frame`
compact-unwind warning; no linker flags or Bazel configuration are changed.

The review follow-up adds four regressions: two pure layout tests and two
production-render/navigation checks. The corrected TUI-filtered suite reports
663 passing tests, and the pure composer layout suite reports five passing
tests. Preferred-column restoration across
shorter rows remains the existing nearest-column behavior, not a new contract.
