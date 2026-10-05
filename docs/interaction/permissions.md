# Permission Selection And Feedback

## Problem

An opaque `custom` badge does not explain effective permissions. Previously,
session shell authorization set that label even when the policy matched a named
preset. The picker also described Auto as asking before edits, although it used
automatic shell execution, and busy submission prevented opening the picker.

## Scope And Integration

This spec owns the picker, feedback, and command availability. Existing runtime
authorization remains defined by [shell approval](../features/shell-approval-policy.md)
and [planning](../features/planning-mode.md). Preset names do not promise OS
isolation beyond the runtime's actual sandbox configuration.

## Contracts

### PERM-01: Describe The Effective Policy

A shared preset catalog defines labels, descriptions, execution mode, shell
approval, sandbox network access, and full-access bypass. Match all dimensions
to select the current preset; `custom` means no exact match, not a running or
error state. The picker always explains execution, shell policy, and sandbox
network state, including for Custom. Selection and rendering use the same order.

The bottom footer is the primary permission status. At a main-content width of
80 columns or more, omit the duplicate permission badge from the activity row.
Below 80 columns, retain the compact activity badge for non-Auto policies as a
fallback. Measure the actual content width after any sidebar, and recompute this
choice on resize. Permission policy and footer content remain unchanged.

| Preset | Execution | Shell approval | Sandbox network | Full-access bypass |
| --- | --- | --- | --- | --- |
| Auto | Execute | Always, retaining escalation checks | Off | Off |
| Accept edits | Execute | Suggestion, retaining read/prefix exceptions | Off | Off |
| Read only | Plan | Suggestion | Off | Off |
| Full access | Execute | Always | On | On |

The initial Execute/Suggestion policy matches Accept edits when sandbox network
is off. If configured network access is on without full-access bypass, it is
Custom; retain that configuration and display its dimensions. File editing in
Execute does not have a separate approval gate. A session shell grant does not grant
network access or full-access bypass; it may match Auto afterward.

### PERM-02: Separate Requested And Effective Permissions

`/permissions` and its alias are available during work. Opening, navigating,
and dismissing the picker preserve the running phase and pending decisions.
Choosing a preset sends an internal typed request to the runtime owner.

- When idle, apply it and report the effective preset.
- During a task, show the requested preset as pending. Keep the executing
  agent and its shared sandbox network flag unchanged until completion.
- Apply the last accepted selection at the completion boundary before queued
  input or automatic continuation starts. Preserve the finishing task's mode
  when interpreting its result.
- Process already-queued user controls before a ready task completion or terminal
  event, so a submitted permission change cannot slip into the following task.
- Choosing the current preset cancels an earlier pending change. Closing the
  picker without selecting leaves an accepted pending request intact.
- Changing permissions never approves, rejects, or clears an existing shell
  or plan decision. A pending change at an automatic plan boundary requires
  an explicit plan decision before implementation continues.
- A failed transport must not display a successful application. A request is
  not an application receipt; effective and pending state come from processing.
- If a task panics and cannot return its agent, reject the pending change with
  an explicit not-applied notice. Normal task errors that return the agent still
  apply the pending choice before any queued continuation.

### PERM-03: Explicit Startup Bypass

`--dangerously-skip-permissions` starts a session in Full access (always allow).
Support the default TUI, `tui`, `resume`, `ask`, `print`, `wire`, and `exec`, with
the flag before or after the subcommand. `exec --full-access` remains compatible.
Reject the flag on administrative commands and ACP rather than silently
pretending to apply it to a different authorization owner.

The flag selects the same local approval/classifier bypass as Full access and
enables sandbox network access. It does not disable OS/container isolation or
answer semantic plan/input decisions. TUI startup applies it after restoration
and before the first task; `/permissions` can change it during the session.
Choosing a thread in the resume picker retains an already selected Full access
policy, including when the restored thread contains pending decisions.
Do not save the flag to provider/configuration defaults. Existing session runtime
records continue to record applied policy as before.

Normal TUI startup retains its existing network-off behavior. A standalone state
constructed from configuration can have a different network flag; the effective
label always follows the assembled session policy.

## Verification

Use production key dispatch, renderer buffers, and completion handling to prove
busy inspection, disabled mutations, exact preset matching, policy descriptions,
deferred changes, replacement/cancellation, queued input, and preserved approvals.
Check narrow and ordinary terminal widths for the selected description and
effective/pending labels. CLI parsing, configuration serialization, and startup
policy checks must cover the bypass flag and unchanged defaults. These checks
do not establish PTY acceptance.

## Source Journal

- [Busy commands and permission controls](../journal/2026-09-17-tui-permission-controls.md)
- [Permission badge placement](../journal/2026-09-17-tui-permission-badge-placement.md)
