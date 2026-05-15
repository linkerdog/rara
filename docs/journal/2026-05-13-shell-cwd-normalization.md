# Shell CWD Normalization

## What changed

- Tightened the `bash` and `pty_start` tool descriptions so the `cwd` field is
  the primary working-directory mechanism at the command decision point.
- Added conservative `bash` payload normalization for simple absolute
  `cd /path && <command>` prefixes when the caller did not already provide
  `cwd` or `program`.
- Added an explicit prompt safety rule that Git branches must not be switched as
  an end-of-task cleanup step unless the user asks for that branch change.
- Added focused unit coverage for normalized, quoted, relative, and explicit
  `cwd` cases.

## Why

RARA already exposed the current working directory through environment context
and tool schemas, but models still tend to emit shell-shaped commands such as
`cd /repo && cargo check`. Normalizing the simple safe case keeps actual tool
execution, summaries, and transcript examples aligned with the `cwd` field
contract.

## Trade-offs

The normalizer intentionally handles only a narrow shell prefix. It does not
rewrite relative paths, semicolon-separated commands, escaped quoted paths, or
commands where `cd` may be deliberate shell state. More aggressive shell parsing
would increase the risk of changing command semantics.

## Remaining work

- Consider the same conservative normalization for PTY only if interactive
  sessions show the same pattern in practice.
