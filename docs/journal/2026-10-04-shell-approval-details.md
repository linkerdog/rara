# Shell Approval Detail Navigation

## Scope And References

[#973](https://github.com/linkerdog/rara/issues/973) reports that the dock shows
at most two detail rows and the transcript only points back to the dock. The
command tail and working directory therefore have no complete viewing path.

Codex `f959e7fc` (`codex-rs/tui/src/bottom_pane/approval_overlay.rs`) renders the
complete shell command with wrapping. Claude Code `4b9d30f`
(`src/components/permissions/BashPermissionRequest/BashPermissionRequest.tsx`)
explicitly requests verbose command rendering. Adapt complete detail rendering
to the existing dock and its four approval scopes.

## Implementation And Boundaries

1. Reserve action rows, then use available dock height for wrapped details.
   PageUp/PageDown and Home/End inspect details with an empty composer; arrow
   selection and explicit approval shortcuts retain their existing semantics.
2. Keep a compact directory summary when space permits and the entire path in
   the scrollable content. Bind offset and viewport bounds to the tool-call ID,
   and clamp on resize. State stores no Ratatui objects.
3. Restore the full transcript approval card. Share sanitization and wrapping
   with the existing display boundary, preserving multiline source text.
4. Verify real key dispatch and rendered pages at narrow widths and short
   heights, including wide characters, long paths, resize, and replacement
   requests. Paging must never send a runtime approval command.

Runtime permissions, command payloads, and persistence formats are unchanged.
This does not require an acknowledgement for every viewed page or claim that
scrolling proves a user read the command.

## Validation

- The old implementation fails both new regressions: later command rows remain
  unreachable after paging, and the transcript contains only the dock hint.
- Six new tests exercise production key mapping/dispatch and rendered buffers:
  every command/path row at 120x24, 80x24, 60x14, 60x10, and 40x8; all four
  choices on every page; fixed directory summary; Home/End and page navigation;
  resize clamping; replacement requests and restored history; composer/overlay
  ownership; and complete transcript details. Navigation emits no runtime
  command before the explicit approval key.
- `cargo test --locked --offline --lib tui::`: 1010 passed, 4 existing tests
  ignored. Existing shell-card assertions now check complete content rather
  than the removed placeholder. No snapshots were regenerated.
- `cargo clippy --locked --offline --all-targets --no-deps -- -D warnings`,
  `cargo fmt --all`, whitespace checks, and the touched-file size audit pass.
- Default `bazel test //:rara_unit_tests` stops during package loading because
  the local external cache lacks `rules_rust//crate_universe` package files.
  No source compilation occurs and no Bazel configuration was changed. The
  exact-head remote default Bazel build/test remains a delivery gate.

## Remaining Boundaries

Automated buffer/key tests do not replace physical-terminal acceptance under
the existing #1010 work. The smallest tested viewport is 40x8; smaller surfaces
still prioritize visible actions, and may have no room for detail content.
