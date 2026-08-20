# Sandbox Execution

## Problem

The default shell sandbox must let ordinary development commands start while
still containing writes and network access. A policy that denies runtime reads
can terminate even `echo`, `date`, `ls`, or `git status` before the command
produces output. If the runtime then exposes only `exit_code = null`, the agent
cannot distinguish a Unix signal from a timeout, launch failure, or policy
denial.

## Scope

- Default `bash` execution through macOS Seatbelt and Linux bubblewrap.
- Workspace-write filesystem and opt-in network capabilities.
- Structured foreground-process termination and sandbox-failure reporting.
- Compatibility with the existing `exit_code`, stdout, stderr, aggregate
  output, and model-preview fields.

## Non-Goals

- Command-name allowlists.
- Treating every non-zero exit or Unix signal as a sandbox denial.
- Sandboxing interactive PTY commands on macOS.
- Domain-aware network proxying.
- Expanding background-task persistence in this phase.

## Architecture

Sandbox policy is capability-based:

- read capability is broad enough to start the shell, dynamic loader, package
  managers, version-control tools, and installed toolchains;
- explicit unreadable roots protect credentials and other sensitive data;
- write capability is allow-only for the workspace, isolated sandbox home, and
  temporary storage;
- network capability remains disabled unless the request or runtime policy
  explicitly enables it.

The macOS backend follows a read-deny/write-allow split: it combines broad
filesystem-read access with explicit configured and canonical sensitive-root
denies. The Linux backend exposes only its mounted runtime roots plus the
workspace, but it must provide the same ordinary-command startup contract.

Process outcome classification is independent from policy classification. The
executor derives a typed termination from the OS status, then attaches a
sandbox failure only when the command was sandboxed and there is concrete
evidence such as a Seatbelt violation or a sandboxed process signal.

## Contracts

### Default command behavior

- Read-only commands such as `echo`, `date`, `ls`, `git status`, and formatter
  checks must be able to start in the default sandbox when their binaries are
  installed and their inputs are readable.
- Writes outside the configured workspace, sandbox home, and temporary roots
  remain denied.
- Sensitive roots such as `~/.ssh` and `~/.aws` remain unreadable even when
  they are nested under another readable path.
- Sensitive-root rules cover both configured and canonical paths so filesystem
  aliases cannot bypass the deny.
- Network access remains denied unless explicitly enabled.

### Foreground result shape

`bash` keeps the legacy nullable `exit_code` and adds `termination`:

```json
{
  "exit_code": null,
  "termination": {
    "kind": "signal",
    "signal": 6,
    "name": "SIGABRT"
  },
  "sandbox_failure": {
    "kind": "sandboxed_process_signaled",
    "backend": "macos-seatbelt"
  }
}
```

Normal exits use `termination.kind = "exit"` with an integer `code`. Unix
signals use `termination.kind = "signal"` with the numeric signal and a stable
name when known. Platforms that provide neither use `termination.kind =
"unknown"`.

`sandbox_failure.kind = "policy_denied"` requires denial evidence in captured
output. `sandboxed_process_signaled` reports that a sandboxed process died by
signal without claiming the policy caused it. Consumers must not translate all
signals into `sandbox_denied`.

### Model and TUI rendering

- A signal termination renders as a signal, not `unknown exit status`.
- Denial hints remain evidence-driven and may recommend dedicated file tools or
  an explicitly escalated shell request.
- Raw stdout/stderr stay available even when a typed termination is present.

## Validation Matrix

| Contract | Check |
|---|---|
| macOS runtime reads | Execute `echo`, `date`, `ls`, and `git status` through a generated Seatbelt profile |
| write containment | Attempt a workspace write and an out-of-workspace write |
| sensitive reads | Attempt reads under configured sensitive roots |
| termination | Unit-test exit, Unix signal, and unknown status serialization |
| denial evidence | Unit-test keyword-backed denial separately from signal-only failure |
| Linux regression | Run the sandbox crate tests for bubblewrap argument construction |

## Operational Notes

The generated macOS profile is per invocation and removed after the command.
Sensitive-root behavior is verified by executing a real denied read through
Seatbelt; profile text ordering alone is not a sufficient security check.

## Open Risks

- Some toolchains need additional macOS IPC capabilities even when filesystem
  reads are correct. Those failures should be added as narrow capabilities with
  regression tests, not solved by disabling the sandbox.
- Background tasks still persist a nullable exit code and do not yet retain the
  full foreground `termination` object.

## Source Journals

- [2026-08-20-agent-tool-reliability](../journal/2026-08-20-agent-tool-reliability.md)
