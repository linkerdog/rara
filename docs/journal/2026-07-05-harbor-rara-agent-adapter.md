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
- Parses RARA JSONL output into Harbor `AgentContext` usage and metadata.
- Preserves parsed context metadata even when `rara exec` exits non-zero.

## Usage

```bash
cargo build --release
PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2 \
  --agent rara_agent:RaraAgent \
  --agent-kwarg binary_path=$PWD/target/release/rara
```

## Validation

```bash
HARBOR_SITE_PACKAGES=$(find "$(uv tool dir)/harbor/lib" -path '*/site-packages' -type d | head -1)
PYTHONPATH="${HARBOR_SITE_PACKAGES}:tools/harbor:." python -m unittest tools.harbor.test_rara_agent
python -m py_compile tools/harbor/rara_agent.py tools/harbor/test_rara_agent.py
```

## Follow-Ups

- Convert RARA JSONL events into full ATIF-compatible trajectory artifacts.
- Run a Harbor Terminal-Bench smoke task with the dynamic adapter and record the
  exact command and resulting artifact paths.
