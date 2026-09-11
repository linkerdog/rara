# 2026-09-11 Prefix Cache Optimization

## Scope And Baseline

The accepted work proceeds in three stages against baseline
`682b22a51362fa010895f0ec0cbbf073f54cb5d5`. It covers inference accounting,
request-prefix correctness, provider capabilities, mode-dependent tools,
cache-aware summaries, compaction, and measured cost/quality comparisons.

## Reference Patterns

- The Anthropic cost-optimization cookbook measures task quality and full task
  cost before changing models or effort. Its explicit system breakpoint makes
  stable context reusable across separate tasks, not only consecutive turns.
- The local Codex reference records individual inference attempts and terminal
  usage, including failed and cancelled attempts, separately from a turn.
- The local Claude Code reference reuses the parent's rendered system, tools,
  model, and thinking configuration for cache-sharing summary requests. Its
  compact fork forbids tool execution without removing schemas from the request.
- The existing DeepSeek decision requires real provider evidence before making
  a stable tool envelope the default. An experimental arm must also measure
  invalid tool calls and task quality.

## Execution Plan

### 1. Complete Accounting And Prefix Regressions

- Input: current provider serializers, retry loops, query reporting, and child
  execution paths.
- Output: task-scoped accounting for main calls, summaries, classifiers,
  descendants, and provider attempts; explicit missing usage; content-free
  request comparisons and deterministic regressions.
- Design: explicit shared accounting handles, owned by a task and propagated
  into children. No process-global mutable collector. Provider token categories
  remain disjoint, and prices are supplied as a dated model/provider tariff.
- Risk: double counting retries, losing late child charges, treating absent
  usage as zero, and leaking prompts through diagnostics.
- Exit: focused tests cover retries, failures, cancellation, child attribution,
  accounting completeness, and actual serialized messages plus non-empty tools.

### 2. Provider And Prefix Corrections

- Input: stage-one observations and serializer regressions.
- Output: only initial system guidance becomes Responses instructions; later
  control messages retain order and authority. Cache profiles distinguish the
  actual endpoint and model. Supported Anthropic paths gain static and advancing
  breakpoints with explicit TTL policy; other protocols receive only supported
  controls.
- Design: explicit provider capabilities and deterministic request assembly.
  Evaluate a session-stable tool envelope behind a selectable experiment while
  keeping runtime permission checks authoritative.
- Risk: authority changes, unsupported request fields, and more invalid tool
  calls. Preserve existing mode visibility unless evidence supports a change.
- Exit: provider request tests, mode-switch and permissions regressions, format,
  compile, and scoped lint checks.

### 3. Summary And Compaction Comparisons

- Input: complete accounting and corrected request prefixes.
- Output: selectable cache-sharing summary and auxiliary-model summary paths;
  task-boundary compaction comparisons including summary and cache-rebuild cost;
  bounded experiments with quality, cost, requests, cache usage, and latency.
- Design: compare identical task fixtures without lowering the default model or
  effort. Preserve recent evidence and tool-call pairing. Use actual provider
  retention contracts rather than a universal idle-time assumption.
- Risk: losing evidence, spending more on a cold auxiliary model, or optimizing
  hit ratio while increasing total cost.
- Exit: offline contract tests plus reviewed live artifacts for the selected
  provider/model and bounded execution ceiling. Inconclusive results stay
  inconclusive; they do not justify changing defaults.

## Validation

Stages 1 and 2 are implemented. Task accounting has explicit physical-attempt
coverage for Chat Completions, Responses, and Bedrock Converse; other backends
remain visibly unobserved. Stage 3 now includes selectable summary/tool
strategies, an offline paired report, and three completed live comparisons.

Focused validation at this checkpoint:

- `cargo test -p rara-observability -p rara-bedrock`: 11 accounting/report tests
  and 7 Bedrock tests pass, including an SDK retry and actual HTTP body capture.
- `cargo test -p rara --lib cache`: 49 tests pass, including real non-empty tool
  schemas, Responses ordering, checkpoint gating, summary request reuse,
  permission enforcement, and compaction plus cache rebuild attribution.
- The child lifecycle regression verifies attribution after the root lease is
  dropped; its actual child inference remains in the original task.
- `cargo test -p rara --test embedded_runtime` passes, including a retained
  task handle and explicit unobserved-backend coverage through the host API.
- All 134 `llm::` tests and all 6 runtime-session integration tests pass,
  including failure receipts, cancellation, busy-session rejection, and
  concurrent independent sessions.
- The final two production-builder tests pass after extending the advancing
  checkpoint to tool-result text. The official OpenRouter SDK permits content
  blocks on tool messages; the regression preserves result role and call ID
  while advancing through consecutive tool calls.
- `cargo clippy -p rara-observability -p rara-bedrock -p rara --lib -- -D warnings`
  and `cargo fmt --all -- --check` pass.
- Default Bazel tests pass for the observability, Bedrock, and tools crates.
  The first invocation executes all three; a final-state verification executes
  Bedrock again and reuses the other two cached results. Bazel's incidental
  lock-format upgrade and pre-existing dependency refresh were removed from
  the patch; no Bazel configuration was changed.
- The `inference_cost_report` executable produces the expected paired report
  from synthetic receipts. This validates the offline interface only.
- `tools/prefix_cache_eval` now supplies the three planned coding fixtures and
  independent standard-library graders. Case 3 keeps a rereadable policy source
  across the declared compaction boundary. Fresh-workspace initialization and
  corpus hashing prevent accidental repair reuse across arms. Missing grader
  receipts remain unknown rather than a quality result.
- All 13 corpus calibration tests pass via
  `python3 -m unittest discover -s tools/prefix_cache_eval -p 'test_*.py'`.
  The checks reject swallowed errors, shifted physical line numbers, changed
  identity semantics, reversed merge order, policy edits, and workspace reuse.
  Black formatting validation passes. These are grader-calibration results,
  not model-generated coding-task outcomes.
- The opt-in provider driver now uses the real agent loop, file tools, mode
  transitions, and explicit compaction, with serial paired arms and content-free
  JSONL receipts. Five offline Rust driver tests pass, including incomplete and
  cancelled billing, pre-call budget refusal, retained costs after grader
  failure, and a complete paired summary/compaction workflow. The paid test is
  ignored during ordinary test runs; live execution is recorded below.
- `cargo test -p rara --lib agent::tests::cache_trial` confirms those five
  offline tests and the single ignored paid test. The final test scope passes
  `cargo clippy -p rara --lib --tests -- -D warnings` as well.
- All 15 `agent::tests::compaction` tests pass after correcting the paid
  driver's per-session deadline. The delayed-summary fixture exceeds the
  ordinary 10 ms test timeout and exercises the production timing selection.
- The root Bazel target now declares the Python corpus as test runfiles; the
  driver resolves it through `TEST_SRCDIR` and `TEST_WORKSPACE` under Bazel and
  the Cargo manifest directory otherwise. Focused Cargo driver tests and Clippy
  pass after this repair. The default invocation
  `bazel test //:rara_unit_tests --test_filter=agent::tests::cache_trial`
  does not forward that filter to this Rust test binary: the actual receipt
  reports 1373 tests passed, one paid test ignored, and zero filtered tests.
  Both grader-executing tests pass in the Bazel sandbox. The generated lock
  format/dependency refresh was saved outside the patch and restored; no Bazel
  configuration change is included.
- The driver requires an explicit dated tariff window and uses a shared budget
  with full-call reservations. The production transport's existing default of
  three send retries is now explicit so the driver's physical-attempt bound
  tracks it. Unsettled reservations prevent all later provider calls.
- [Live official pricing](https://api-docs.deepseek.com/quick_start/pricing/)
  was refreshed on 2026-09-11: `deepseek-flash` is the
  canonical Flash name, and the selected Pro alias has an announced routing
  change on September 14. The cache profile recognizes the canonical Flash
  name; no prices are embedded in production code. Refresh both model identity
  and tariff before any paid comparison.
- The test linker reports a macOS large `__eh_frame` warning. This is not a
  Rust compiler or Clippy diagnostic; it remains visible in validation output.

The first paid pilot ran on 2026-09-11. Its first coding phase passed, and
main requests returned complete usage with subsequent cache reads. Compaction
then inherited the 10 ms unit-test timeout, cancelling one Flash attempt without
usage. Known pilot charges total USD 0.01378828; including the unknown attempt
at its full model-specific context/output bound gives a USD 0.33327628 upper
bound. The pilot is unpaired and inconclusive. Its raw receipt is retained in
`/private/tmp/prefix-cache-live-20260911-0642/summary-pilot.jsonl`; the bound and
remaining allocation are in `pilot-accounting-review.json` in that directory.

The driver now selects production compaction timing per session; ordinary
timeout tests retain their fast deadline. An offline delayed-summary backend
exercises this boundary. The missing pilot usage stays unpriced, and its upper
bound is deducted before any new run. No strategy default has been promoted.

## Initial Live Summary Comparison

The corrected driver completed two paired repetitions of case 3, alternating
arm order. The main model was `deepseek-v4-pro`, configured reasoning effort
`max`, with 4096 output tokens per request. The auxiliary alias
`deepseek-v4-flash` currently resolves to V4.1 Flash. Both arms used the same
fixture, mode-filtered tools, compaction boundary, and external grading phases.
All 50 physical requests returned complete usage.

| Summary route | Whole-task passes | Total task cost (USD) | Requests | Rejected tools | Cache reads / input tokens |
|---|---|---:|---:|---:|---:|
| Auxiliary | 2 / 2 | 0.138691924 | 24 | 1 | 118016 / 166519 |
| Cached main | 0 / 2 | 0.154163856 | 26 | 0 | 157824 / 205364 |

Cached-main total task cost was 11.16% higher in this small sample. Both
candidate failures occurred in the first grading phase, before compaction;
all four post-compaction phases passed. Therefore the pass-rate difference
cannot establish a causal quality regression from the summary route. It does
prevent this trial from supporting a default promotion. Higher cache reading
alone also failed to establish a task-cost advantage.

The artifact is `summary.jsonl` under
`/private/tmp/prefix-cache-live-20260911-0642/`, with the tariff, code hashes,
configuration metadata, and separate aborted-pilot receipt. This successful
comparison incurred USD 0.29285578 in total; the aborted pilot remains a
separate setup cost with unknown usage and a retained upper bound.

## Initial Live Tool-Schema Comparison

Two paired repetitions of all three cases completed on the same main model,
effort, tariff, and request limits. Both arms retained history in this comparison.
All 110 requests returned complete usage; every arm began with zero cache reads.
The baseline used two distinct tool-schema fingerprints per session, while each
candidate session retained one fingerprint across mode transitions.

| Tool schemas | Whole-task passes | Total task cost (USD) | Requests | Rejected tools | Cache reads / input tokens |
|---|---|---:|---:|---:|---:|
| Mode-filtered | 5 / 6 | 0.304543800 | 54 | 1 | 307200 / 413804 |
| Session-stable | 6 / 6 | 0.203630416 | 56 | 1 | 393344 / 442797 |

The candidate's total cost was 33.14% lower. Cache reads increased from 74.24%
to 88.83% of input. Net input charges fell by USD 0.071648984; lower output
token usage contributed another USD 0.029264400. The complete task-cost result
therefore includes generation variability as well as cache-related input savings.
The candidate did not increase observed rejected tool calls, and Plan/Review
workspace checks passed. The sole baseline task failure was in case 3 phase 2.

This supports the opt-in stable-schema strategy for this measured setup. The
corpus exposes two real file tools, not the runtime's full extension/tool set;
six samples per arm do not establish broader quality or cost parity. Keep the
strategy selectable rather than promoting a global default from this result.
Raw evidence is `tools.jsonl` alongside `tools-report.json` and
`tools-cost-components.json` in the execution artifact directory.

## Initial Live Compaction Comparison

Two paired repetitions of case 3 compare retained history with explicit
auxiliary-model compaction at the declared task boundary. All 45 physical
requests returned complete usage. Both arms use mode-filtered tools and the
same main model, reasoning effort, fixture, and external grading phases.

| History policy | Whole-task passes | Total task cost (USD) | Requests | Rejected tools | Cache reads / input tokens |
|---|---|---:|---:|---:|---:|
| Retained | 1 / 2 | 0.108922880 | 22 | 0 | 133120 / 171924 |
| Boundary compacted | 2 / 2 | 0.103976300 | 23 | 0 | 102400 / 147643 |

Total task cost was 4.54% lower with compaction in this sample. The candidate
bill includes USD 0.004938900 for two summaries. Both first requests after
compaction had zero cache reads, with 6374 and 6503 input tokens respectively;
their rebuild charges and all subsequent requests are included. In contrast,
the retained arm's first phase-two requests read 7296 and 7168 cached tokens.
The retained arm's failure occurred in phase two; all phase-one grades passed.

The small net saving includes variation before the compaction boundary as
well as subsequent inference. This is evidence for evaluating deliberate task
boundaries, not for a universal compaction threshold or a quality guarantee.
Hot history remains intact until pressure or explicit compaction. Receipts and
phase costs are in `compaction.jsonl`, `compaction-report.json`, and
`compaction-phase-costs.json` in the execution artifact directory.

## Combined Live Evidence

The [checked-in aggregate report](../../tools/prefix_cache_eval/results/2026-09-11-deepseek.json)
includes the selected profile, tariff, corpus hash, all three comparison
reports, and raw-receipt hashes. Raw receipts and measured source hashes remain
in the author's local execution artifact directory; they are not checked in.

The three comparisons contain 20 paired task samples and 205 requests with
complete usage, costing USD 1.013929176. Including the aborted pilot's known
charges gives USD 1.027717456. Its single missing usage receipt remains
unpriced: adding the reviewed USD 0.319488 maximum charge gives an overall
USD 1.347205456 upper bound, within the USD 20 execution ceiling. The ceiling
was an execution default, not a user-supplied amount. These are tariff-derived
inference charges, not a reconciled provider account statement.

| Comparison | Baseline p50 / p95 (ms) | Candidate p50 / p95 (ms) |
|---|---:|---:|
| Summary route | 145797 / 207065 | 201865 / 227222 |
| Tool schemas | 84218 / 142238 | 89653 / 128260 |
| History policy | 94850 / 118161 | 88499 / 102493 |

These are observed task-duration order statistics, not population latency
estimates. `combined-report.json` retains costs per successful task, including
failed tasks in the numerator, alongside all three reports. The metadata and
source hashes identify the measured runtime after the timeout correction;
the subsequent Bazel runfiles repair changes test resource lookup only.

Keep auxiliary summaries as the default. Keep stable tool schemas selectable:
they are promising for the measured two-tool workload, but the full tool set
and additional providers need their own evidence before default promotion.
The measured comparisons complete stage three for the selected profile; they
do not establish a global minimum cost or statistical quality equivalence.

## Live Comparison Protocol

First validate corrected prefix serialization and capability contracts through
actual wire-request regressions. Then run three paid strategy comparisons.
Run comparisons sequentially, keeping model, effort, tool implementations,
source fixtures, grader, and output limits identical between paired arms:

1. Mode-filtered versus session-stable tools, with Plan/Execute/Review transitions.
2. Auxiliary summary versus cached-main summary at the same boundary.
3. Retained history versus explicit task-boundary compaction, including the
   summary, the rebuilt prefix, and the next task in both bills.

Use deterministic coding fixtures with an external grader: an off-by-one range
fix, a tool-result/error-path repair, and a two-step task requiring a previously
read constraint after compaction. Include successful and failing tasks in the
cost numerator, alternate arm order, isolate run state, and retain warm-up
charges separately from steady-state cache ratios. A tool-free cache probe is
insufficient to evaluate the tool-schema and compaction arms.

Pin provider endpoint, actual model, region where relevant, dated tariff,
repetitions, request/output bounds, and spending ceiling before running. Drain
admitted descendants and post-turn work before reading the final ledger. Missing
usage or an unsupported billing category makes the comparison inconclusive.
Require paired task quality and invalid-call evidence before changing defaults;
report savings only as a point estimate for the tested workload.

## Follow-Ups

- Before promoting stable tool schemas globally, evaluate the full runtime
  tool set and longer tasks with more repetitions and independently refreshed
  model routing and prices.
- Expand physical-attempt instrumentation and known-zero billing contracts only
  when evaluating additional providers; do not infer them from API compatibility.
