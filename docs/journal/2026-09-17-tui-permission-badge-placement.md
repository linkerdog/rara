# Permission Badge Placement

## Summary

Keep permission status in the bottom footer on ordinary and wide terminals.
The activity row retains its compact non-Auto badge only below 80 columns of
main-content width. Resizing re-evaluates placement without changing policy.
The contract is recorded in [PERM-01](../interaction/permissions.md#perm-01-describe-the-effective-policy).

## Reference And Plan

- Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`,
  `bottom_pane/footer.rs`: footer presentation uses width-dependent fallbacks.
- Official [Claude Code status line documentation](https://code.claude.com/docs/en/statusline)
  distinguishes persistent status from footer hints and supports avoiding a
  duplicate mode indicator. Adopt only the presentation principle here.
- Use the existing main-content width passed to the view builder; keep this
  decision in display data rather than runtime permission state.
- Verify rendered activity/footer placement across wide, boundary, narrow,
  and wide-again sizes, then review affected snapshots.

## Validation

- The new production-renderer regression failed against the old view builder:
  the 180-column activity row still contained `perm=full-access`.
- After the change, the same running session passed resize checks at 180, 80,
  79, 60, and 180 columns. The bottom footer retained permission status at every
  size; the upper badge appeared only below the breakpoint.
- `cargo test --locked --lib tui::`: 650 passed. Two snapshot changes were
  reviewed: startup warning and provider-picker background text reclaim the
  space previously used by the duplicate badge.
- `cargo check --locked`, strict workspace Clippy, formatting, and whitespace
  checks passed. Live SSH terminal acceptance is not claimed.
