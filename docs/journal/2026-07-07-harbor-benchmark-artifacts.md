# Harbor Benchmark Artifact Guidance

## Summary

Tightened the Harbor adapter contract after a Terminal-Bench
`sparql-university` smoke run exited successfully but did not create the
required `/app/solution.sparql` artifact.

## Changes

- Wrapped Harbor task instructions with generic non-interactive benchmark
  guidance.
- Made named output files explicit required artifacts in that guidance.
- Passed the benchmark workspace to `rara exec` through both Harbor's command
  cwd and RARA's own `--cwd` option.
- Kept the guidance task-agnostic: it does not include benchmark answers,
  hidden verifier behavior, or task-specific shortcuts.

## Validation

- `python -m unittest tools.harbor.test_rara_agent`
- `python -m py_compile tools/harbor/rara_agent.py tools/harbor/test_rara_agent.py`
- `cargo fmt`

