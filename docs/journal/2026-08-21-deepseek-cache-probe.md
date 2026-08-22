# DeepSeek Cache Probe

## Summary

RARA now exposes content-free per-request model observations through its Rust
embedding facade and includes an opt-in paired probe for measuring DeepSeek
automatic prefix-cache behavior through the official chat-completions API.

## Upstream Patterns Reviewed

- Codex models token accounting as structured last-request and cumulative
  usage facts. RARA adopts the per-request boundary needed by embedding
  consumers while retaining its existing cumulative agent counters.
- Claude Code diagnoses prompt-cache breaks with bounded component hashes and
  structured metrics. RARA adopts component hashes but narrows the artifact to
  SHA-256 values and counts; it does not retain raw prompt diffs or global
  mutable detector state.

## What Was Built

- Added `QueryReport` and `ModelTurnReport` for main-model usage, cache
  accounting, duration, finish reason, and optional request fingerprints.
- Added `EmbeddedRuntime::query_with_report` without changing existing event
  variants or query methods.
- Added DeepSeek request fingerprints derived from the production
  chat-completions serializer. Fingerprints contain backend-scoped salted
  SHA-256 values, per-component counts, and no raw content.
- Enabled DeepSeek streaming usage chunks with
  `stream_options.include_usage=true`.
- Split OpenAI-compatible wire request/response logic into a focused protocol
  module so every touched production source file remains below 1000 lines.
- Added a paired AB/BA live probe with isolated state, alternating arm order,
  warm-up exclusion, disabled tools and extension execution, a fixed official
  base URL, random non-identifying DeepSeek `user_id` cache partitions, and
  bounded output tokens.
- Added a dual opt-in example executable that writes a non-overwriting JSONL
  report.

## Decisions And Trade-Offs

The existing runtime event enums were left unchanged because adding fields to
public variants would break downstream pattern matches and external protocol
adapters. The report is a query return value instead.

Cache accounting is optional in the report. The usage parser must observe an
explicit provider cache field before zero hit/miss values are considered
measured data.

The live probe disables all model tools. This preserves the production prompt
assembly and provider serializer while preventing a measurement prompt from
causing local actions. Production tool-schema stability remains observable
through ordinary `query_with_report` fingerprints.

The first turn of each arm is excluded from the aggregate comparison because
its unique experiment scope intentionally starts cold. Alternating AB/BA order
reduces a fixed time-order bias, but it cannot eliminate provider load or cache
eviction noise.

## Verification

The implementation is covered by offline request-body, fingerprint,
perturbation, aggregation, and embedded-runtime tests. The root library suite
completed 1,338 tests, the 85 existing OpenAI-compatible regressions passed,
all-target checking and warning-denying Clippy passed, and the example dry run
stopped before credential lookup or runtime construction. A live provider call
is not part of repository validation and was not made for this checkpoint.

```bash
cargo fmt --all -- --check
cargo test -p rara deepseek_cache_probe -- --nocapture
cargo test -p rara llm::openai_compatible::cache_observation -- --nocapture
cargo test -p rara --test embedded_runtime -- --nocapture
cargo check -p rara --all-targets
cargo clippy -p rara --all-targets -- -D warnings
```

On macOS, test linking emitted the existing compact-unwind `__eh_frame` size
notice; all tests completed successfully.

The example remains offline unless both cost gates are supplied:

```bash
cargo run --example deepseek_cache_probe -- --pairs 3 --turns-per-arm 3
```

A credentialed operator can explicitly run:

```bash
DEEPSEEK_API_KEY=... cargo run --release --example deepseek_cache_probe -- \
  --live \
  --acknowledge-cost \
  --pairs 5 \
  --turns-per-arm 3
```

## Remaining Work

- Run the probe with an authorized credential and review the JSONL artifact.
- Record the environment, model, provider response fields, stable-versus-busted
  delta, and any inconclusive result without checking credentials or raw
  session state into the repository.
- Use ordinary production query reports to study environment, mode, policy,
  and tool-schema churn separately from the action-free live probe.
