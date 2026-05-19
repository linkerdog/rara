# Codex 0.131 Status Line Borrowing

## What changed

RARA now keeps the bottom footer status dense enough to show the active
permission mode and bash approval mode even when the runtime is idle.

The specific borrowed behavior comes from Codex Rust 0.131.0, which called out
status-line visibility for approval and permission state. RARA already had the
detailed `/status` command and activity badges, so this checkpoint keeps the
implementation narrower: the footer always includes `perm=<mode>` and
`approval=<mode>` before live token, cache, or compaction statistics.

## Trade-offs

- The footer is no longer blank in an idle workspace without repository
  context. This is intentional because permission state is operationally useful
  before a prompt is submitted.
- The change avoids duplicating the full `/status` view. The footer remains a
  compact scan target rather than a second status panel.
- Agent execution mode still stays in the existing activity line and composer
  hints, because the release note item being mirrored is specifically approval
  and permission visibility.

## Validation

- Updated bottom pane footer tests to assert the idle permission and approval
  labels.
- Updated the busy footer test to keep live token statistics while preserving
  the new permission summary.
