# Harbor RARA Agent Adapter

## Summary

Added a repo-local Harbor adapter that can be dynamically loaded through
Harbor's `--agent module.path:ClassName` import-path support.

The adapter lives at `tools/harbor/rara_agent.py` and exposes
`rara_agent:RaraAgent`.

## Scope

- Uploads a locally built RARA binary into the benchmark container.
- Invokes `rara exec --json` with Harbor trial metadata and stdin-provided task
  instructions.
- Writes `rara-exec.jsonl` and the final assistant message under
  `/logs/agent`.
- Preserves the `rara exec` exit code while teeing JSONL output, so Harbor does
  not treat a failed agent run as a successful zero-reward verifier run.
- Validates the uploaded binary during agent setup and reports host/container
  architecture mismatches before the task runs.
- Parses RARA JSONL output into Harbor `AgentContext` usage and metadata.
- Preserves parsed context metadata even when `rara exec` exits non-zero.

## Usage

```bash
cargo build --release
PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2 \
  --agent rara_agent:RaraAgent \
  --agent-kwarg binary_path=$PWD/target/release/rara
```

When Harbor uses Linux Docker containers, `binary_path` must point to a Linux
RARA binary. A macOS `target/release/rara` build will fail setup with an
`Exec format error`.

For a single-task smoke run:

```bash
PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2 \
  --task terminal-bench/sparql-university \
  --agent rara_agent:RaraAgent \
  --agent-kwarg binary_path=$PWD/target/release/rara
```

## Validation

```bash
HARBOR_SITE_PACKAGES=$(find "$(uv tool dir)/harbor/lib" -path '*/site-packages' -type d | head -1)
PYTHONPATH="${HARBOR_SITE_PACKAGES}:tools/harbor:." python -m unittest tools.harbor.test_rara_agent
python -m py_compile tools/harbor/rara_agent.py tools/harbor/test_rara_agent.py
```

The single-task smoke run `terminal-bench/sparql-university` confirmed that
`cwd=/app` is required for Docker execution and that verifier reward `0.0` can
mean the agent produced no task artifact. The adapter now defaults to `/app` and
preserves `rara exec` failures through the JSONL tee pipeline.

A follow-up smoke run exposed a host/container binary mismatch: the local
`target/release/rara` was a macOS arm64 binary while the task ran in a Linux
container. The adapter now probes `rara --version` after upload and fails setup
with an actionable Linux-binary diagnostic.

## Follow-Ups

- Convert RARA JSONL events into full ATIF-compatible trajectory artifacts.
- Run a Harbor Terminal-Bench smoke task with the dynamic adapter and record the
  exact command and resulting artifact paths.
