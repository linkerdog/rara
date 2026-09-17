# TUI Commands

## Problem

Repeated aliases increase discovery noise. Independently maintained help and
dispatch logic can advertise missing commands or execute display syntax as data.

## Scope And Non-Goals

This specification owns built-in slash commands in the terminal composer. It
does not change CLI subcommands, runtime tool discovery, or persisted settings.

## Architecture

`command/specs.rs` owns discoverable command metadata, matching, and parsing.
`keymap.rs` and `event_dispatch.rs` translate selection into submission;
`submit.rs` routes local commands to `runtime/commands.rs`. Runtime maintenance
requests use `RuntimeClientPort`.

## Contracts

### CMD-01: One Discoverable Entry Per Action

The empty command palette and the Commands help tab show canonical names once,
sorted alphabetically. Compatibility aliases are accepted when typed; typing a
complete alias in the palette ranks its canonical command first.

| Canonical command | Result | Compatibility spelling |
| --- | --- | --- |
| `/approval` | Toggle the bash approval policy; derive the matching permission preset | None |
| `/clear` | Reset local transcript, live display, and local queued/pending presentation state; retain the backend session | None |
| `/compact` | Request one history compaction pass | None |
| `/connect` | Open provider connection setup | None |
| `/context` | Inspect assembled context and its sources | `/memory` |
| `/goal` | Show or manage the current thread goal | None |
| `/help` | Open General, Commands, and Runtime help tabs | None |
| `/mcp` | Show configured MCP server status | None |
| `/mem` | Configure the builtin memory connection | None |
| `/model` | Open the unified model picker | None |
| `/permissions` | Open the permission preset picker | `/permission` |
| `/plan` | Enter read-only planning mode | None |
| `/quit` | Persist local runtime state and leave the terminal UI | `/exit` |
| `/resume` | Open the recent thread picker | `/threads` |
| `/review` | Start review of current local changes when an agent is available | None |
| `/skills` | Inspect loaded skills and invocation availability; read-only | None |
| `/status` | Inspect runtime, configuration, and context status tabs | `/runtime` |
| `/tasks [task_list_id]` | Show the current task list, or switch it when an ID is supplied | `/task-list` |

`/mem` and `/context` have different purposes. `/approval` controls only the
bash policy; `/permissions` chooses a broader preset. Their distinct behavior
is a reason to retain them, not evidence of duplicate commands.

`/auth`, `/base-url`, `/login`, `/logout`, `/models`, and `/dream` are not
built-in TUI commands. Provider setup is reached through `/connect` and `/model`.

### CMD-02: Help Syntax Is Not Executable Data

Selecting an item runs its canonical bare command. Optional placeholders such
as `[task_list_id]` are display documentation and must never become arguments.
Users enter arguments explicitly in the composer, for example `/tasks review`.
Selecting `/tasks` therefore shows the current list and never creates or selects
a list named `[task_list_id]`.

### CMD-03: Local Submission And Errors

- A parsed built-in command is handled locally, without becoming an LLM prompt.
- An unknown slash command produces an explicit notice and is not submitted
  as ordinary task input.
- During a running task, allow `/help`, `/status`, `/context`, `/permissions`,
  `/skills`, `/mcp`, `/tasks` without an argument, `/goal` without an argument,
  and `/quit`, including aliases. Inspection must preserve the running phase.
- Commands that replace or mutate the active runtime wait until the task ends.
  The palette and Commands help show the same disabled reason that submission
  enforces. `/tasks <id>` and goal mutations remain unavailable while busy.
  Ordinary text follows [RUN-01](runtime-feedback.md#run-01-submission-and-queueing).
- `/goal` argument semantics are owned by [thread goals](../features/thread-goals.md).
- `/tasks` argument semantics are owned by [shared task lists](../features/shared-task-lists.md).

### CMD-04: Help Matches Reachable Behavior

The General page describes current user actions and key handling. The Commands
page uses the same canonical metadata as the palette. Help must not list removed
entry points, claim that `/permissions` immediately cycles permissions, or
describe internal file-editing tools as user keyboard actions.

### CMD-05: Skill Inspection Does Not Pretend To Change Runtime Policy

The skills overlay displays the runtime snapshot. Space does not mutate a
local checkbox or claim to enable/disable a runtime skill. Automatic invocation
and manual-only availability are reported as status. A future enablement editor
must first have a runtime-owned update path and readback; local presentation
mutation alone is not a successful setting change.

## Validation Matrix

| Contract | Observable check |
| --- | --- |
| CMD-01 | Render the palette and Commands help; verify canonical entries and absence of duplicate aliases; submit complete aliases |
| CMD-02 | Select `/tasks` through key dispatch; assert that the active list is unchanged and the status is rendered |
| CMD-03 | Busy inspection preserves progress; mutations and aliases use the same policy in palette, help, and submission |
| CMD-04 | Open `/help` through submission and inspect the production-rendered General page |
| CMD-05 | Render runtime-projected skill status; press Space and verify no local toggle or runtime command |

## Open Risks

- `/clear` does not start a new runtime session. New-session semantics require
  a separate lifecycle design; do not assume another client's `/clear` contract.
- Command parsing and execution still have separate match tables. New entries
  must be checked through dispatch, not only registry enumeration.

## Source Journals

- [TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
- [Busy commands and permission controls](../journal/2026-09-17-tui-permission-controls.md)
