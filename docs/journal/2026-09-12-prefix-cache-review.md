# Prefix Cache Review Corrections

## Summary

The second review of PR #868 found that a separate Python process still exposed
verifier sources through the host filesystem and allowed descendants to outlive
normal completion. It also found gaps in partial-cost reporting and cache
capability declarations.

## Decisions and implementation

- The grader stages the observation-only worker outside the verifier directory.
  macOS uses a deny-by-default Seatbelt profile with runtime/fixture read access,
  no network, and no process creation. Linux uses Bubblewrap with a minimal
  filesystem, separate user/network/PID namespaces, and no capabilities. The
  environment is empty. A process-group teardown runs on every exit before
  policy revalidation. Unsupported platforms, including Windows, fail closed.
- A sandbox preflight must succeed before the paid driver admits model calls.
  CI installs Bubblewrap and runs the adversarial Python calibration suite.
- Pricing retains known output and cache charges when another cache category is
  absent. It does not guess ordinary-input charges from an incomplete breakdown.
  Running attempts contribute their latest cumulative counters without being
  marked complete. Final HTTP error bodies mark received usage as terminal;
  missing categories still prevent complete billing.
- Bedrock reports retention control whenever its explicit cache TTL is enabled.
- The authentication suggestion is a false positive: production interpolation
  uses the configured secret, and the HTTP fixture verifies its fake bearer key
  on the initial request and retries. Production authentication is unchanged.

## Reference patterns

The implementation follows the OS boundary used by the local Codex
`codex-rs/linux-sandbox/src/bwrap.rs` and
`codex-rs/sandboxing/src/seatbelt.rs`, and the
[Claude Code sandbox runtime](https://github.com/anthropics/sandbox-runtime).
For these pure-function fixtures, descendant creation is unnecessary on macOS;
Linux uses the [Bubblewrap PID namespace](https://github.com/containers/bubblewrap)
to contain even descendants that start a new session. The general runtime's
sandbox allows broader workspace operations and is deliberately not changed.

## Validation

Focused validation commands:

```sh
cargo fmt --all -- --check
cargo test -p rara-observability --lib
cargo test -p rara --lib inference_ -- --test-threads=1
cargo test -p rara --lib agent::tests::cache_trial
cargo clippy -p rara --lib --tests -- -D warnings
python3 -m unittest discover -s tools/prefix_cache_eval -p 'test_*.py'
python3 tools/prefix_cache_eval/run.py preflight
```

Local checks passed: 13 observability tests, 15 inference/provider tests,
5 offline driver tests (the paid test remains ignored), and 24 Python tests.
Clippy completed with warnings denied. The macOS test linker reports the same
large `__eh_frame` warning as the prior review checkpoint.

The Python calibration covers correct/incorrect repairs, absolute/adjacent/
symlink verifier access, inherited environment and network isolation, delayed descendants
across normal exit/failure/timeout, and unavailable/unsupported sandboxes.
An outer sandbox that denies starting the inner OS sandbox cannot produce valid
grades; run the tests from a host that supports this boundary. No sandbox bypass
or default-strategy change is introduced.

The macOS Bazel sandbox rejects nested Seatbelt. The offline driver integration
test checks that unavailable grading preserves cost accounting with unknown
quality; when preflight succeeds, it still requires both real grades to pass.
The standalone Python calibration unconditionally requires a working sandbox,
so this conditional integration assertion cannot replace execution coverage.
No Bazel configuration or test sandbox policy is changed.

Ubuntu CI probes namespace creation before testing and, only if blocked, loads
an AppArmor profile attached to `/usr/bin/bwrap` on the ephemeral runner. This
follows Ubuntu's [per-application user namespace policy](https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces)
without changing global sysctls. Both the actual grader preflight and the full
Python calibration are mandatory before the Cargo suite, so setup failures have
direct diagnostics and cannot hide behind unknown integration grades.

## Additional review checkpoint

A later review reproduced a wrong `window` implementation passing after it
replaced `__main__.observe.__code__`. The parent now admits only the fixture's
restricted pure-function source language and freezes accepted source in the
worker directory before execution. Observer mutation and forged full JSON
observations are rejected before process creation. Reference repairs and
incorrect ordinary implementations still exercise the real observer. The task
prompts and corpus identity change with this explicit submission contract; this
does not claim support for arbitrary Python programs or revive earlier grades.

The same review found that missing Bedrock aggregate writes dropped known TTL
totals from inclusive input, and absent TTL detail fields became known zeros.
Normalization now preserves those known totals and distinguishes absent details
from a reported exhaustive list. AWS documents an explicitly empty list as no
creation, so that case remains known zero. Anthropic aggregate-only creation
also retains unknown TTL categories and cannot produce complete billing.

The new OpenRouter suggestion is not applicable: its official
[usage accounting contract](https://openrouter.ai/docs/cookbook/administration/usage-accounting)
always includes usage in the final streaming chunk and deprecates both usage
request parameters. Both repeated authentication suggestions remain contradicted
by the actual source and HTTP capture test. No production change follows those
three suggestions.

Additional local validation passed: 27 Python calibration tests, 3 Bedrock
accounting tests, 15 inference/provider tests, and 5 offline driver tests
(the paid test remains ignored), plus Clippy with warnings denied.

## Context identity and future categories

The final review also found that future TTL categories could inherit the generic
write tariff and that a cached summary could outlive its backend or execution
context. Unknown Bedrock/Anthropic creation details now keep the generic category
unknown while retaining recognized counters. A private captured-prefix wrapper
binds reuse to backend object identity, runtime mode, and visible tool schemas;
any mismatch selects the auxiliary path. A weak backend reference avoids keeping
replaced providers alive and distinguishes identical labels at different endpoints.
Focused regressions cover unknown TTLs and real compaction after backend,
tool-schema, and Execute-to-Plan/Review changes under both schema policies.

The local Codex compaction path constructs requests from the current turn's
model, instructions, and client session. This implementation adapts that
pattern by retaining the captured request only while its current context agrees.

Four Bedrock accounting tests and three cache-experiment tests passed locally;
the context-change test exercises six distinct changes with real compaction.
The latest local Bazel attempt timed out during Cargo manifest splicing before
any tests started. Its generated lock-file format change was discarded. Linux
CI for `b57c4a9c` passed Bazel and all 27 strict Python calibration tests; Cargo's
full suite encountered the unrelated LSP early-exit status assertion. The next
commit requires its own CI receipts.

## Follow-ups

The historical September 11 quality aggregate remains ineligible for strategy
promotion. Per-phase candidate sources were not retained, so this review does
not revalidate the old grades or rerun paid trials. Existing evaluation follow-up
work remains in `docs/todo.md`.
