# Shell Capability Fallback Guidance

## Summary

The default prompt now requires command availability checks before relying on optional shell
utilities. It keeps `rg` as the preferred shell search utility when available and directs the
agent to use an equivalent available or POSIX fallback otherwise.

## Background

TerminalBench runs inside task-provided containers. A failed task showed that an agent can stop
after assuming a utility such as `python3` exists. Preinstalling tools in the benchmark adapter
would mutate the task environment and weaken the evaluation contract.

## Key Decisions

- Check optional commands with `command -v <command>` before depending on them.
- Prefer dedicated tools when they provide the required capability.
- Use available or POSIX fallbacks when practical.
- Do not infer a package manager or install missing dependencies unless the user explicitly asks
  for that environment change.

## Validation

- `cargo test -p rara-instructions default_prompt_checks_command_availability_before_shell_fallbacks`
- `cargo fmt --check`
- `cargo check`
