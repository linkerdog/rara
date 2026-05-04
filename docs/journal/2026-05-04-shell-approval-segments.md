# Shell Approval Segment Boundaries

## Context

Codex treats shell control operators as approval boundaries. A command string
joined with `|`, `&&`, `||`, `;`, or a subshell boundary is split into segments,
and each segment is evaluated independently for sandbox and approval purposes.

RARA already had a segment splitter for read-only classification, but reusable
prefix approval still checked whether the full normalized shell string started
with a saved prefix. That allowed an approved prefix such as `git push` to cover
an unrelated chained segment.

## Change

- Added segment-level reusable-prefix evaluation for legacy shell command
  strings.
- Kept read-only multi-segment commands auto-allowable in suggestion mode.
- Allowed a mixed command only when every segment is read-only or covered by an
  approved prefix.
- Rejected prefix reuse for conservative shell syntax gaps: redirection,
  substitutions, variable expansion, wildcards, subshell grouping, and leading
  environment assignments.
- Preserved exact-command replay for shell commands that cannot produce a
  reusable prefix. The replay key must match the full command summary and does
  not approve additional segments.
- Updated the agent loop to call the segment-aware approval helper instead of
  applying `any(prefix)` to the whole command.
- Documented the control-plane boundary: TUI, ACP, and Wire adapters may submit
  approval decisions, but the runtime remains responsible for applying prefix,
  exact-command, and segment-level policy checks.

## Validation

- `cargo test bash::tests::approval_prefix`
- `cargo test approved_bash_prefix_does_not_auto_allow_unapproved_shell_segments -- --nocapture`
- `cargo check`
