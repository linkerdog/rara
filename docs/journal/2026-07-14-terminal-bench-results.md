# Terminal-Bench Observed Results

## Summary

This journal is the evidence-backed record of observed Terminal-Bench task
outcomes for RARA's Harbor adapter. A task is confirmed as passed only when the
Harbor verifier reports reward `1.0`; a successful RARA process exit alone is
not sufficient.

## Results

| Task | Outcome | Evidence | Notes |
| --- | --- | --- | --- |
| `terminal-bench/regex-log` | Passed | Harbor verifier reward `1.0` | The run recorded RARA exit status `0` and an authoritative verifier success. |
| `terminal-bench/overfull-hbox` | Failed | Final task artifact violated an edit constraint | The agent removed the LaTeX warnings, but also changed `an` to `a` when only synonym substitutions were allowed. The task therefore did not pass validation. |
| `terminal-bench/sparql-university` | Passed (artifact pending) | User-reported successful benchmark run | Earlier attempts were inconclusive because of CA and provider setup failures. Record the Harbor verifier artifact for this successful rerun when its job path is available. |

## Recording Rules

- Report verifier reward separately from RARA's exit status and final message.
- Use `Passed` for a confirmed official verifier reward of `1.0`; append
  `(artifact pending)` when the successful outcome is reported before its
  Harbor verifier artifact is available in the repository.
- Use `Failed` when the verifier rejects an otherwise completed task artifact.
- Use `Inconclusive` for adapter, provider, credential, environment, or harness
  failures that prevent a valid task attempt.
- Record the Harbor run command, dataset revision, RARA revision, provider,
  model, and links to JSONL and verifier artifacts for every future result.

## Evidence

- `docs/journal/2026-07-09-harbor-provider-env.md` records the `regex-log`
  verifier reward and the earlier inconclusive `sparql-university` setup attempts.
- The successful `sparql-university` outcome was reported during the
  2026-07-14 evaluation checkpoint; its Harbor job path and verifier reward
  have not yet been added to this repository.
- `docs/journal/2026-07-09-headless-constraint-validation.md` records the
  `overfull-hbox` constraint violation.

## Follow-Ups

- Re-run `overfull-hbox` with the current full-access and constraint-validation
  guidance, then append the authoritative verifier result.
- Use `regex-log` as the first smoke-regression task before adding a more
  constraint-heavy task to the smoke subset.
- Add the successful `sparql-university` Harbor job path and verifier artifact
  to this record.
