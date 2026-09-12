# Prefix Cache Task Corpus

These offline tools prepare deterministic coding tasks and grade their public
behavior. They never call a model, obtain credentials, or produce a cache-savings
claim. Python 3.10 or later is sufficient; there are no package dependencies.

| Case | Contract | Workflow |
|---|---|---|
| 1 | Correct half-open pagination bounds, errors, and unchanged inputs | Plan, Execute, Review |
| 2 | Fail on invalid JSON records with physical line numbers and original causes | Plan, Execute, Review |
| 3 | First-wins, case-sensitive record identity and ordered merge | Repair, optional compaction boundary, second task, Review |

## Prepare And Grade

Run from the repository root. Pick a fresh workspace for every arm and repetition;
`init` refuses to overwrite an existing directory.

```sh
python3 tools/prefix_cache_eval/run.py show --case 3
python3 tools/prefix_cache_eval/run.py export
python3 tools/prefix_cache_eval/run.py init --case 3 --workspace /tmp/cache-case-3-baseline-0
python3 tools/prefix_cache_eval/run.py grade --case 3 --phase 1 --workspace /tmp/cache-case-3-baseline-0
python3 tools/prefix_cache_eval/run.py grade --case 3 --phase 2 --workspace /tmp/cache-case-3-baseline-0
python3 -m unittest discover -s tools/prefix_cache_eval -p 'test_*.py'
```

Only `init` output files belong in the model's workspace. `show` provides the
ordered prompts, execution modes, grading phases, and eligible compaction
boundary for the host. Keep this directory, reference repairs, and graders
outside the model's tools. Execute the grader within the surrounding isolated
task environment; its subprocess timeout and Python `-I` flag do not create a
filesystem or network sandbox. A grade exits with status 1 when the task fails.
Missing or invalid grader receipts produce `passed: null`, which must remain
ungraded in the task comparison. A timed-out candidate is a failed task under
the fixed grader time limit.

The grader uses independent contract examples and boundary sweeps, not generated
model tests or a model's self-report. Calibration checks require all starters
to fail, known repairs to pass, and plausible incorrect repairs to fail.
Candidate modules execute in `worker.py`; the parent verifier never imports
them or exposes its expected results in the worker request. JSON observations
include returned values, input mutation, object-origin indices, and error
categories. The verifier checks these observations and caps accepted output at
2 MB. A worker's own pass/fail receipt is invalid. The worker is staged away
from verifier sources and executes in an OS sandbox with an empty environment.
macOS uses `/usr/bin/sandbox-exec` with a read allowlist and no process creation;
Linux requires `/usr/bin/bwrap` and permitted user/PID/network namespaces.
The Linux sandbox mounts only runtime paths, the staged worker, and the fixture.
Verifier source cannot be read through absolute paths or fixture symlinks.
All exit paths drain the process group before protected inputs are rechecked.
Windows and hosts that cannot establish this boundary return an unknown grade;
there is no unsandboxed fallback. An outer sandbox must permit starting this
inner sandbox. Check the local environment without calling a provider:

```sh
python3 tools/prefix_cache_eval/run.py preflight
```

The paid driver requires a successful preflight before the first model call.
Case 3 also rejects changes to the policy source. Its first phase deliberately
does not require the second task's implementation.
The protected policy is checked before and after worker execution, including
timeouts and missing receipts. Corpus hashing includes the worker and isolation
implementations. OS sandbox policies and Python are additional host dependencies.

## Paired Trial Contract

Use the same corpus hash, model/effort, tool implementations, source inputs,
grading phases, and output limits for both arms. Run all declared turns in one
session, setting the host execution mode before each prompt. Record and reject
workspace mutations during Plan and Review. The runtime's rejected-tool counter
is an additional metric, not a replacement for checking mode semantics.

For case 3, compare retained history against compaction only at the declared
boundary. Grade phase 1 before compaction and phase 2 after the second task.
The policy source remains available for rereading; those extra model/tool
requests count toward the task cost. Keep all phases and the final review in
one `InferenceTask` ledger. Wait for admitted descendants and post-turn work.
The task passes only when every required phase passes and mode constraints hold.

Combine that external pass/fail result with the terminal ledger into
`InferenceExperimentSample`, pairing by `case_id` and repetition. Use
`InferencePriceTable::compare_tasks` or `cargo run --example inference_cost_report`
to price both arms. Include warm-up, summary, retry, failure, and rebuild charges;
do not use the older Harbor main-turn token totals as a complete task bill.
Pin the corpus hash in run metadata alongside the provider/model, tariff
revision, arm order, and approved spending ceiling.
Record the Python version too. Verify provider-specific warm-up charges and
subsequent cache reads, including explicit writes where separately billed,
before interpreting cache effects; a corpus
run below the provider's caching threshold is inconclusive for that comparison.

## Opt-In Provider Driver

`src/agent/tests/cache_trial/driver.rs` connects this corpus to the real agent
loop, file tools, execution modes, explicit compaction, and task accounting.
Its paid test is ignored by default. Offline tests exercise the same driver
with deterministic responses; they do not contact a provider.

The paid path currently requires the selected DeepSeek profile in the existing
configuration and the official endpoint. It uses the configured main model and
effort, and the configured auxiliary model or the runtime's existing inferred
auxiliary route. Credentials
are read by the host only and are never written into trial artifacts. Each
arm receives a fresh workspace, backend instance, and DeepSeek `user_id`.
Provider cache isolation must still be confirmed from actual receipts.

| Comparison | Baseline | Candidate | Cases |
|---|---|---|---|
| `tools` | Mode-filtered schemas | Session-stable schemas | 1, 2, 3 |
| `summary` | Auxiliary summary | Cached-main summary | 3; both compact at the same boundary |
| `compaction` | Retain history | Compact at the declared boundary | 3 |

These compare strategies on the corrected runtime. Measuring an old-versus-new
serializer additionally requires a separately pinned baseline build.

Before a paid run, prepare a JSON tariff file with two fields:

- `table`: an `InferencePriceTable` with a dated `revision` and exactly one
  price entry for each selected model. Entries contain `provider: "DeepSeek"`,
  the exact `model`, and numeric USD-per-million-token rates for `input`,
  `output`, `cache_read`, `cache_write`, `cache_write_5m`, and `cache_write_1h`.
- `validity`: `valid_from_unix_ms` and `valid_until_unix_ms` bounding one verified
  tariff and model-version window. A new call is refused unless its full
  90-second deadline fits inside this window.

Refresh the official [model and tariff table](https://api-docs.deepseek.com/quick_start/pricing/)
before filling these values. Peak/off-peak prices and model alias changes can
invalidate an otherwise identical comparison. Reports price received tokens
under the supplied tariff; they are not a provider invoice. A missing receipt
or a cancelled remote request cannot be assumed free.

After authorizing a ceiling and verifying the sandbox preflight, run:

```sh
export CACHE_TRIAL_ALLOW_PAID=yes
export CACHE_TRIAL_MAX_USD="${APPROVED_RUN_CEILING_USD:?set the authorized allocation}"
export CACHE_TRIAL_PRICES="${VERIFIED_TARIFF_JSON:?set the verified tariff file}"
export CACHE_TRIAL_OUTPUT="${NEW_TRIAL_JSONL:?set a new output path}"
export CACHE_TRIAL_COMPARISON=summary
export CACHE_TRIAL_REPETITIONS=2
cargo test -p rara --lib agent::tests::cache_trial::run_paid_prefix_cache_comparison -- --exact --ignored --nocapture --test-threads=1
```

`CACHE_TRIAL_PYTHON` optionally selects Python. Each invocation has its own
ceiling; when running several comparisons, deduct prior charges and retained
reservations from the total authorized allocation. Stop the sequence on an
incomplete bill. The driver allows 1-3 repetitions, at most six model turns per
prompt, and 4096 output tokens per request. It shares one budget across all
arms in an invocation and runs them serially, alternating order by repetition.

Before polling a model call, it reserves the maximum charge for a full 1M
context, the configured output limit, and the production retry/fallback bound.
The integer-microdollar reservation is deliberately conservative. Complete
receipts release unused funds; partial receipts, cancellation, or missing
attempts retain the reservation and block later calls. This does not cancel a
request already admitted by the provider or enforce the account's external
billing policy. The supplied tariff must cover every potentially charged token.

The output is created without overwriting an existing file. It contains run
metadata, corpus hash, Python version, content-free request fingerprints,
per-task grades and full accounting, followed by a paired comparison. Each
sample is flushed before continuing. Grader failures retain incurred model
costs and an unknown quality result. Prepared workspaces are retained at the
path printed by the driver for reviewing actual model changes; they are
separate from the content-free report.

Live execution evidence is recorded in the
[implementation journal](../../docs/journal/2026-09-11-prefix-cache-optimization.md).
The [2026-09-11 aggregate report](results/2026-09-11-deepseek.json) includes
all three paired comparisons and the dated tariff; raw receipts remain local.
Its quality figures predate worker isolation and post-execution policy checks.
They retain their historical corpus hash and are explicitly unvalidated for
strategy promotion; the per-phase sources needed for a complete regrade were
not retained. Inference cost receipts are unchanged.
These small fixtures calibrate the mechanism; they do not establish statistical
quality parity on a broad workload. The driver explicitly selects production
compaction timing per session, avoiding the fast unit-test timeout.
