# Terminal-Bench Observed Results

## Summary

This journal is the evidence-backed record of observed Terminal-Bench task
outcomes for RARA's Harbor adapter. A task is confirmed as passed only when the
Harbor verifier reports reward `1.0`; a successful RARA process exit alone is
not sufficient.

## Results

| Task | Outcome | Evidence | Notes |
| --- | --- | --- | --- |
| `terminal-bench/regex-log` | Passed | Harbor verifier reward `1.0` | The original run and the 2026-08-23 Terminal-Bench 2.1 rerun both recorded RARA exit status `0` and an authoritative verifier success. |
| `terminal-bench/sqlite-with-gcov` | Passed (artifact pending) | User-reported successful benchmark run | This confirms the full-access PATH handling validated in the accompanying Harbor checkpoint. Record the Harbor verifier artifact when its job path is available. |
| `terminal-bench/headless-terminal` | Failed | Harbor verifier reward `0.0` | Five completed reruns passed six of seven checks but missed background-process behavior through the generated terminal interface. Job `2026-08-24__12-30-48` confirmed that a fresh reviewer without the first pass's evidence repeats familiar checks and can drift into unrelated robustness work. The reviewer now receives an evidence delta and awaits rerun. |
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
- `docs/journal/2026-08-22-terminal-bench-headless-profile.md` records the
  content-addressed Terminal-Bench 2.1 `regex-log` rerun, artifact hashes, and
  verifier reward `1.0`.
- The successful `sparql-university` outcome was reported during the
  2026-07-14 evaluation checkpoint; its Harbor job path and verifier reward
  have not yet been added to this repository.
- The successful `sqlite-with-gcov` outcome was reported during the
  2026-07-15 evaluation checkpoint; its Harbor job path and verifier reward
  have not yet been added to this repository.
- `docs/journal/2026-07-09-headless-constraint-validation.md` records the
  `overfull-hbox` constraint violation.
- `docs/journal/2026-07-15-terminal-bench-service-validation.md` records the
  `headless-terminal` service-validation failure and adapter correction.
- `docs/journal/2026-08-23-terminal-bench-headless-lifecycle.md` records the
  completed rerun, the remaining lifecycle gap, and the follow-up correction.

## Follow-Ups

- Re-run `overfull-hbox` with the current full-access and constraint-validation
  guidance, then append the authoritative verifier result. This is the next
  scheduled single-task test.
- Re-run `headless-terminal` with the evidence-delta verification-and-repair
  pass and active reasoning controls, then append the authoritative verifier
  result.
- Add a more constraint-heavy task to the Terminal-Bench 2.1 smoke subset and
  repeat selected tasks before reporting a suite-level score.
- Add the successful `sparql-university` Harbor job path and verifier artifact
  to this record.
- Add the successful `sqlite-with-gcov` Harbor job path and verifier artifact
  to this record.
