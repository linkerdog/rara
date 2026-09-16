# Goal Resume and Permission TUI Interaction

## Scope

Align RARA's local goal-resume and shell-permission interactions with the
observable Codex and OpenCode contracts while retaining RARA's session-scoped
runtime boundary.

## What Changed

- `/goal resume` now changes a paused or blocked goal to pursuing only when the
  session is idle and its runtime agent is available, then immediately starts a
  dedicated goal-continuation turn.
- Creating a goal from `/goal <objective>` follows the same idle-continuation
  path, so a local goal starts work without waiting for an unrelated prompt to
  finish first.
- Goal-continuation prompts no longer render as synthetic user (`You`)
  transcript entries. The TUI reports the continuation through its normal goal
  activity state instead.
- Added a narrow `ContinueGoal` runtime command so the TUI controller can ask
  the session runtime to start that hidden continuation without owning an
  `Agent` or task services.
- Shell approval keeps one horizontal bottom-pane decision surface. Left/Right
  and `h`/`l` follow the visual action order, existing vertical navigation and
  numeric shortcuts remain compatible, and `Esc` rejects the command.
- Reworded approval actions to expose their scopes: once, matching prefix,
  current session, and reject.
- Required a model that marks a goal blocked to finish its current turn with a
  concise user-facing blocker report and the condition for safe resumption.
- Removed two implicit privilege escalations: choosing `Full Access` no longer
  approves a command that is already pending, and choosing the session shell
  grant no longer enables global full-access or network access.
- Made the legacy `/approval` toggle session-scoped too, so switching bash to
  always-allow cannot silently promote the session to `Full Access`.
- Reset shared approval selection whenever a plan-approval interaction opens,
  preventing a stale fourth shell option from selecting an invalid plan action.

## Why

The old local goal command only recorded the objective, so its automatic loop
could not begin until an unrelated query finished. The old permission surface
also displayed a horizontal choice row while requiring vertical navigation, and
its broad approval labels hid global capability changes. Codex resumes active
goals through an idle continuation path and avoids starting when the thread is
not idle; Claude Code applies the same explicit continuation-or-stop pattern
to token-budget loops. OpenCode keeps permission choices in a single session
interaction with left/right selection and explicit confirmation. RARA now
preserves those interaction properties without copying unrelated runtime
architecture.

## Trade-Offs

RARA's wire-compatible `ShellApprovalDecision::Suggestion` still represents a
rejection internally. Renaming that protocol variant would be a cross-surface
compatibility change, so the TUI maps it through a named option helper and never
exposes its internal name to users.

`Allow this session` changes bash approval behavior only. Escalated operations
may still require a distinct approval because their sandbox capability is not
part of a command-scope grant; users must choose the global profile explicitly
when they intend to widen that boundary.

## Validation

- Focused command test for blocked-goal resume starting a hidden continuation.
- Focused input-control test for all shell decision indices, including reject.
- TUI keymap, scope-preservation, plan-selection-reset, and render regression
  tests.
- `cargo fmt --all`
- Focused `cargo test --locked` targets followed by workspace `cargo check`
  and Clippy.
