# Harbor Full-Access PATH Validation

## Summary

Harbor's explicit `rara exec --full-access` mode now bypasses the
auto-permission classifier as well as interactive bash approval. Benchmark
guidance also requires PATH-dependent work to be checked from a fresh
non-interactive process.

## Background

The `terminal-bench/sqlite-with-gcov` run compiled SQLite with gcov support,
but did not score because the verifier could not resolve `sqlite3` on PATH.
The agent's attempt to create `/usr/local/bin/sqlite3` was denied by the
auto-permission classifier despite the adapter selecting full access. It then
mistook a command-local PATH export and shell startup-file update for verifier
visibility.

## Scope

- Skip the auto-permission classifier when the caller explicitly selects full
  access.
- Keep the classifier and approval policy unchanged for normal sessions.
- Add generic benchmark guidance and adapter coverage for non-interactive PATH
  validation.

## Key Decisions

- Full access is valid only when selected explicitly by the caller; for Harbor,
  the task container is the external isolation boundary.
- The guidance is task-agnostic. It does not name a benchmark command or encode
  verifier behavior beyond the general distinction between shell-local and
  fresh-process environment state.

## Validation

- `cargo test --locked agent::tests::planning::full_access_mode_bypasses_auto_permission_classifier_denials -- --nocapture`
- `PYTHONPATH=$PWD/tools/harbor /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest tools.harbor.test_rara_agent`
- `cargo fmt --check`
- `cargo check --locked`

## Follow-Ups

- Re-run `terminal-bench/sqlite-with-gcov` and record the Harbor verifier
  artifact in the Terminal-Bench observed-results journal.
