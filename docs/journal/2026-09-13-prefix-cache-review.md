# Prefix Cache Receipt and Capture Corrections

## Summary

Review follow-up for PR #868 narrows Bedrock model matching, preserves partial
usage through arithmetic failures, and prevents failed requests from publishing
a cached-summary prefix.

## Decisions

- Match exact Bedrock IDs after one supported regional prefix. Preserve the
  adapter's selected TTL support rather than enabling a family by suffix. The
  [AWS cache model table](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html)
  provides the versioned IDs; adding newer support is separate from this repair.
- The inference usage type introduced in this PR now records whether its input
  total is incomplete. Overflow retains the base input lower bound and all
  independently known output/cache categories, while ordinary-input cost stays
  unknown. Legacy receipts without the new flag retain their existing meaning.
  Contradictory creation totals leave generic writes unknown instead of losing
  the entire receipt.
- Capture the exact main request only after a successful response, and clear
  prior capture before sending. A failure therefore routes later compaction to
  the auxiliary path; a subsequent success enables capture again.
- The recovery regression also found that leading compact-summary messages were
  incorrectly removed along with the generated system prompt. Only the first
  generated prompt is separated now; system messages from history remain in the
  captured prefix and are serialized exactly once.

The local Codex client publishes `LastResponse` only on `ResponseEvent::Completed`.
The local Claude Code compaction path retains explicit failure handling around
its summary call. This repair applies the successful-response boundary to the
existing capture experiment without changing default summary routing.

## Validation

Focused checks cover rejected model suffixes, overflow near `u64::MAX`,
contradictory creation details, preservation of known costs, and real compaction
after a failed request followed by a successful retry.

The original model-suffix, usage-overflow, and failed-capture regressions failed
before their fixes. Local validation now passes 10 Bedrock tests, 13 accounting
tests, 4 cache-experiment tests, 17 provider/summary cases, and the additional
serialization compatibility case. Clippy completed with warnings denied.
The macOS test linker retains its existing large `__eh_frame` warning.

```sh
cargo test -p rara-bedrock --lib
cargo test -p rara-observability --lib
cargo test -p rara --lib inference_ -- --test-threads=1
cargo test -p rara --lib agent::tests::cache_experiment
cargo clippy -p rara --lib --tests -- -D warnings
cargo fmt --all -- --check
```

## Follow-Ups

No paid comparisons are rerun. Historical quality grades remain ineligible for
strategy promotion. The existing Linux-only resource boundary remains required
for executing grading candidates.
