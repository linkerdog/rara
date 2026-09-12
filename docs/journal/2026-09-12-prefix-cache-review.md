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

## Follow-ups

The historical September 11 quality aggregate remains ineligible for strategy
promotion. Per-phase candidate sources were not retained, so this review does
not revalidate the old grades or rerun paid trials. Existing evaluation follow-up
work remains in `docs/todo.md`.
