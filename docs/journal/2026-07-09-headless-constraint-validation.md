# Headless Constraint Validation

## Summary

RARA now gives headless benchmark runs and normal execute-mode turns stronger
guidance to validate explicit task constraints before reporting completion.

## Background

The `terminal-bench/overfull-hbox` task compiled successfully and removed all
overfull hbox warnings, but the final `input.tex` changed `an` to `a` while
only synonym substitutions were allowed. The edit tool exposed the diff, but the
agent's working validation only tracked the primary LaTeX goal and did not keep
the allowed-substitution invariant as a completion check.

## Scope

- Harbor benchmark instructions now state that constraints such as allowed edit
  sets, untouched files, exact formats, and allowed substitution lists must be
  verified directly before finishing.
- The default execute-mode validation guidance now treats explicit task
  constraints as validation requirements rather than prose guidance.

## Validation

- `python -m unittest tools.harbor.test_rara_agent`
- `cargo test -p rara-instructions default_system_prompt_includes_workflow_standards -- --nocapture`

## Follow-Ups

- Consider a structured post-edit verifier hook if future benchmark failures
  show the prompt guidance is not enough for constraint-heavy editing tasks.
