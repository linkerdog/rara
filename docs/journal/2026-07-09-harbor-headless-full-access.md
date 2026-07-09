# Harbor Headless Full Access

**Date**: 2026-07-09
**Context**: Terminal-Bench runs through `rara exec` and the Harbor adapter.
**Result**: Harbor now runs headless RARA with explicit full-access shell mode inside the task container.

---

## Summary

`terminal-bench/overfull-hbox` exposed a headless execution gap after provider credentials were fixed. The model correctly tried to compile `main.tex` with `pdflatex`, but the bash tool first attempted sandboxed execution. The task container did not have `bubblewrap`, so sandboxed bash failed with:

```text
sandboxed command execution is unsupported on platform linux (sandbox unavailable: install bubblewrap/bwrap)
```

The model then requested escalated shell permissions. In headless Harbor execution there is no interactive approval surface, so RARA stopped with:

```text
headless exec completed without a final assistant message
```

## Scope

Changed:

- `rara exec` accepts `--full-access`.
- `run_exec_command` sets `Agent.full_access_mode` when the flag is present.
- The Harbor adapter passes `--full-access` by default because Terminal-Bench already isolates the run inside a task container.
- The Harbor benchmark instruction tells the model to request escalated sandbox permissions for shell commands, so missing `bwrap` does not force a failed first sandbox attempt.
- Harbor adapter tests now assert that generated `rara exec` commands include `--full-access`.
- CI now has a focused `harbor-adapter` workflow for the Python adapter tests and `app_cli::tests`.

Not changed:

- TUI permission defaults.
- Non-Harbor `rara exec` behavior unless callers opt into `--full-access`.
- The bash tool sandbox implementation.

## Key Decisions

- Keep the permission expansion explicit at the `rara exec` CLI boundary instead of inferring from environment variables.
- Apply full access in the Harbor adapter because benchmark containers are already the isolation boundary and many Terminal-Bench tasks require shell commands for validation.

## Validation

Failure log inspected:

- `jobs/2026-07-09__13-32-01/result.json`
- `jobs/2026-07-09__13-32-01/overfull-hbox__NELn3Jb/agent/rara-exec.jsonl`
- `jobs/2026-07-09__13-32-01/overfull-hbox__NELn3Jb/exception.txt`

Commands:

```bash
PYTHONPATH=$PWD/tools/harbor /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest tools.harbor.test_rara_agent
cargo test --locked app_cli::tests
git diff --check
```

## Follow-Ups

- Re-run `terminal-bench/overfull-hbox` after the fix lands to confirm that `pdflatex` runs inside the task container without an approval pause.
