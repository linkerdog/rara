# Harbor Provider Environment

**Date**: 2026-07-09
**Context**: Terminal-Bench runs through the Harbor RARA adapter.
**Result**: The adapter now prepares container CA certificates and forwards provider credentials without putting API keys in the command line.

---

## Summary

The Harbor adapter failed in two stages when running `terminal-bench/sparql-university`:

- RARA could panic during startup inside the task container because `reqwest` could not load system CA certificates.
- After CA setup was fixed, RARA could still complete through the mock backend when the benchmark container did not receive a real provider configuration, leaving `/app/solution.sparql` missing.

The adapter now installs or refreshes CA certificates during agent setup, before validating the uploaded RARA binary. It also supports provider/model/base URL kwargs and maps a selected provider API key from the Harbor host process environment into `RARA_API_KEY` for the container execution environment.

## Background

Harbor runs the uploaded RARA binary inside a task container with its own filesystem and environment. The adapter intentionally sets a fresh `RARA_HOME` under `/logs/agent/rara-home`, so host-local RARA config files are not automatically available in the benchmark container.

That isolation is useful for reproducible trials, but it means provider credentials must be forwarded explicitly. Without credentials or provider selection, RARA can use the mock backend and produce a successful-looking final message without writing the benchmark artifact.

## Scope

Changed:

- `tools/harbor/rara_agent.py`
  - installs `ca-certificates` for common Linux package managers when the bundle is missing;
  - accepts `provider`, `model`, `base_url`, and `api_key_env` kwargs;
  - infers a provider from supported host API key environment variables when no provider kwarg is set;
  - forwards the API key through `RARA_API_KEY` in the environment, not through command-line flags;
  - fails the agent phase if a completed turn still reports `Mock Response:`.
- `tools/harbor/test_rara_agent.py`
  - covers CA setup ordering;
  - covers provider flag construction without command-line API key exposure;
  - covers host environment API key forwarding;
  - covers mock backend completion rejection.

Not changed:

- RARA core provider configuration semantics.
- Terminal-Bench task definitions.
- Host RARA config file upload behavior.

## Key Decisions

- Keep API keys out of the generated shell command so Harbor job logs do not capture secrets.
- Prefer explicit `--agent-kwarg provider=...` and `--agent-kwarg model=...`, while still allowing provider inference from host environment variables for local runs.
- Fail early on mock backend completion so the job reports the real setup issue before verifier output collapses into missing artifact assertions.

## Validation

```bash
PYTHONPATH=$PWD/tools/harbor /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest tools.harbor.test_rara_agent
git diff --check
```

Manual log inspection used:

- `jobs/2026-07-08__20-45-55/sparql-university__w3PTeRy/agent/rara-exec.jsonl`
- `jobs/2026-07-09__11-04-40/sparql-university__CnKTBtr/agent/rara-exec.jsonl`

## Follow-Ups

- Run a full Harbor Terminal-Bench job with a rotated provider key and verify that `/app/solution.sparql` is created before the verifier starts.
