# Provider Cache Observability

## Problem

RARA preserves provider-cache-sensitive request prefixes, but a stable request
shape alone does not prove that a remote provider reused cached tokens. Cache
measurement also needs to remain usable from the Rust embedding API without
adding raw prompts or provider-specific fields to the stable runtime event
protocol.

## Scope

- Return per-request token usage, latency, finish reason, and content-free
  request fingerprints from an embedded query.
- Request DeepSeek streaming usage through its official chat-completions API.
- Provide an opt-in paired DeepSeek experiment that compares stable and
  deliberately invalidated first-message prefixes.
- Emit JSONL measurement artifacts without prompt, response, credential, tool
  name, or workspace-path content.
- Attribute all inference attempts for a task, including summaries, classifiers,
  descendants, retries, failures, and cancellation. Keep logical calls distinct
  from transport attempts and retain usage observed before a stream failure.
- Price disjoint uncached input, cache read, cache write, and output categories
  using an explicit provider/model tariff. Missing usage or a missing tariff
  makes the task cost incomplete, never zero.

## Non-Goals

- Guaranteeing that DeepSeek retains or reuses any request prefix.
- Emulating provider cache edits or assuming all compatible endpoints cache.
- Sending live provider requests from tests or ordinary runtime startup.
- Changing existing `AgentEvent`, `SessionEvent`, or runtime-control protocol
  variants.
- Treating latency differences alone as evidence of a cache hit.

## Architecture

### Embedded query report

`EmbeddedRuntime::query_with_report` is an additive API over the normal agent
loop. It returns a `QueryReport` containing one `ModelTurnReport` for each main
model request made by the query. A query may contain multiple model turns, so a
single aggregate would lose retry and tool-continuation boundaries.

Each turn reports:

- model label and elapsed request time;
- provider token usage when present;
- cache hit and miss tokens only when the provider response contains usable
  cache-accounting fields;
- finish reason;
- an optional backend-produced request fingerprint.

Existing typed events remain unchanged. Consumers that do not request the
report retain their current source and protocol behavior.

### Task accounting contract

Task accounting uses an explicitly propagated, task-owned handle. Concurrent
root tasks have independent ledgers; descendants retain their originating task
even when they finish after the parent's first response. A report taken while
work is active is a snapshot, not a final bill. Callers can keep the accounting
handle to read the completed report after background work finishes.

Each logical call has a purpose and an opaque agent identity. Each actual model
request has its own attempt identity, model/provider identity, elapsed time,
terminal status, and optional token accounting. Transport retries and fallback
models produce additional attempts. A dropped in-flight request is reported as
cancelled; provider usage already received is retained. A backend without
attempt instrumentation is explicitly reported as incomplete coverage.

Final usage receipts and intermediate stream counters are distinct. Partial
charges contribute to known cost but cannot make total cost complete. A terminal
snapshot means currently admitted work has drained; hosts must finish scheduling
post-turn extraction, evaluators, and children before treating it as a task bill.

Provider adapters normalize total input tokens to include cache reads and
writes. They preserve separate creation categories, including short and long
TTL writes where reported. Pricing must not charge cache reads or writes again
as ordinary input. Unknown cache categories cannot be treated as known zeros.
Prices have an explicit revision and provider/model match; no global model-name
guess supplies a price for a custom endpoint.

Billing identities distinguish custom endpoint hashes, OpenAI API versus Codex
subscriptions, and Bedrock regions. A supplied tariff must match the reported
identity; a gateway named like a direct provider does not inherit its price.

Request-prefix regressions compare production serialization, including tools
and request options. Message-only tests with an empty tool manager cannot prove
that mode switching preserves a cacheable prefix.

### Responses instruction ordering

Only the initial consecutive system messages populate top-level `instructions`.
System controls appended after user or assistant history remain chronological
`system` input messages. They keep instruction authority without moving ahead
of earlier observations. Appending a control must preserve existing input items,
tools, reasoning options, and top-level instructions byte for byte.

Cache capability selection must check the endpoint identity and model family.
A custom gateway does not inherit capabilities from its API shape or model name.
Profiles describe supported behavior, not evidence of a cache hit. Unknown
retention never authorizes a time-based claim that the cache has expired.

Verified OpenRouter Anthropic routes use two explicit content-block checkpoints:
the last leading system block and the newest eligible conversation block,
including a tool-result text block during consecutive tool calls. Tool call IDs
and result roles are preserved; no synthetic user message is inserted. This
keeps a reusable static checkpoint even across independent tasks. The default is
five minutes; `with_anthropic_cache_ttl` can select one hour for known pause-heavy
workflows. Both checkpoints use the same TTL. No top-level automatic control is
sent, because it would exclude some OpenRouter routes. DeepSeek, OpenAI, custom
gateways, and unsupported model routes never receive Anthropic controls.

Native Bedrock uses Converse `cachePoint` blocks for verified Claude models and
records physical requests through SDK transmit/attempt hooks, including internal
retries. Its input counter excludes reads and writes; task accounting adds them
once and preserves the `cacheDetails` TTL breakdown. Gemini Code Assist and
coding subscriptions remain conservative until their own cache contract is verified.

Bedrock long TTL is separately validated: the documented one-hour option is
enabled only for Claude Sonnet 4.5, Opus 4.5, and Haiku 4.5 model IDs, including
regional inference-profile prefixes. Unknown models and unsupported TTLs fail
before a request is sent. Model support must be refreshed from AWS rather than
inferred from a newer family name.

| Adapter | Physical-attempt coverage | Completeness limit |
|---|---|---|
| Chat Completions | Main, summary, classifier, transport retry and fallback | Requires terminal usage with all cache billing categories. |
| Responses | Main, summary, classifier and transport retries | Partial stream usage is retained but never a final bill. Subscription tariffs are separate. |
| Bedrock Converse | SDK transmit and attempt hooks, including SDK retries | Requires inclusive normalized usage and the matching regional tariff. |
| Other or custom backend implementations | Logical calls are recorded | Missing attempt instrumentation remains unobserved; total cost stays incomplete. |

Old OpenAI or compatible responses that omit a write category remain incomplete
in this accounting adapter until their precise endpoint/model billing contract
can establish a known zero. Capability declarations alone never fill in usage.

### Selectable summary and tool experiments

Host sessions can select a fixed tool schema snapshot and a cached-main summary
strategy. Both defaults retain existing mode filtering and auxiliary summaries.
The stable schema option does not widen execution permissions: tools forbidden
in the active mode remain rejected by the runtime.

The cached-main strategy captures the rendered system, conversation, tools, and
execution-mode request options. Compaction reuses its matching prefix, keeps the
main model, and appends a text-only summary instruction. Tool schemas remain
visible, but any generated tool call makes the summary fail; no summary tool is
executed. A range or overflow retry that no longer matches the captured prefix
uses the auxiliary path and is accounted as such. The comparison includes this
fallback, summary attempts, and the following cache rebuild request. A strategy
is not promoted based on hit ratio alone.

### Cost and quality comparison artifact

`InferencePriceTable::compare_tasks` pairs externally graded task samples by
opaque case ID and repetition. It reports full task cost, cost per passed task
(including charges from failed tasks), request counts, cache reads/writes,
rejected tool requests, and p50/p95 task duration. Quality regression is checked
per paired case. Missing prices, partial usage, unobserved calls, active children,
duplicate cases, and missing partners prevent a complete cost comparison.
Missing grades remain unknown. Savings are point estimates and never prove
statistical significance or trigger a default-strategy change.

`cargo run --example inference_cost_report` reads a JSON object with `prices`
and `samples` from standard input and emits the comparison. This executable
does not make model requests. Real trials must use identical task fixtures and
graders, alternate arm order, account for warm-up and cache rebuilds explicitly,
and record the selected provider/model, tariff revision, and spending ceiling.

The [offline task corpus](../../tools/prefix_cache_eval/README.md) provides
three coding contracts, ordered mode transitions, a two-task compaction boundary,
and external graders. Each arm starts from a fresh workspace. Grader calibration
requires known repairs to pass and plausible incorrect repairs to fail. Corpus
and grader hashes belong in run metadata; grade receipts contain no workspace
path, source code, or model output. Missing grader receipts remain ungraded.

An ignored, opt-in provider driver binds that corpus to the real execution
loop and file tools. It compares mode-dependent schemas, summary routes, or
task-boundary compaction one factor at a time. Paid calls require a selected
supported profile, an explicit ceiling, and a tariff validity window. A shared
per-run budget reserves the bounded worst-case charge before polling each
logical call, including its physical retry/fallback bound. Only complete
receipts release unused reservations. Cancellation or incomplete accounting
blocks subsequent calls; grader failure preserves costs and an unknown grade.
The grader runs within the surrounding task sandbox, not a sandbox created by
the driver. Actual provider artifacts remain the gate for changing defaults.

### Content-free request fingerprints

The DeepSeek backend fingerprints the exact logical JSON body constructed by
the production serializer. Transport-only `stream` and `stream_options` fields
are excluded because they do not change the model-visible prefix.

The report stores SHA-256 values for:

- the complete logical request;
- leading system messages;
- all messages and each individual message;
- all tools and each individual tool;
- remaining request options.

JSON object keys are canonicalized before hashing. SHA-256 inputs include a
random backend-instance salt that is never reported; an opaque hash-scope ID
indicates which fingerprints are comparable. The report stores only hashes,
the scope ID, and counts. At most 256 per-message and 256 per-tool hashes are
retained. They permit a bounded common-prefix comparison within that scope
without reconstructing or logging prompt content.

### DeepSeek streaming usage

DeepSeek requests continue to use the official OpenAI-compatible
`/chat/completions` interface. Streaming requests set:

```json
{
  "stream": true,
  "stream_options": {
    "include_usage": true
  }
}
```

RARA parses `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens` from the
provider's final usage chunk. Other OpenAI-compatible endpoint kinds do not
inherit this option unless their contract is verified separately.

### Paired live probe

`run_deepseek_cache_probe` runs independent stable-prefix and cache-busted
arms. It uses the standard runtime prompt assembly and DeepSeek serializer, but
disables tools and extension/hook execution to prevent the measurement model or
workspace automation from performing local actions.

For each pair:

1. Each arm receives fresh RARA state and a unique experiment scope.
   The scope is sent as a random, non-identifying DeepSeek `user_id`, which the
   official API uses for KV-cache isolation.
2. The stable arm prepends the same neutral marker to the first system message
   for every request.
3. The cache-busted arm changes that marker before every request.
4. Both arms execute the same bounded scripted turns.
5. Even-numbered pairs run stable then busted; odd-numbered pairs run busted
   then stable.
6. The first request in each arm is excluded as warm-up.

The backend is fixed to the official DeepSeek base URL, thinking is disabled,
`user_id` contains only a generated UUID, and `max_tokens` is bounded. The
example executable requires both `--live` and `--acknowledge-cost`; without both
flags it performs no network request.

The probe writes session state below a unique run directory. Its JSONL output
contains run metadata, content-free samples, and an aggregate summary. Callers
own retention or removal of the isolated state directory.

## Contracts

| Contract | Detail |
|---|---|
| Additive library API | Existing embedded query and event APIs remain source-compatible. |
| Profile extension | `ProviderCacheProfile` adds an explicit-cache capability; exhaustive external struct literals must include it or use a constructor. |
| Exact serializer | Fingerprints derive from the same request builder used for the provider call. |
| Content-free artifact | Reports contain hashes, counts, usage, durations, and labels only. |
| Accounting honesty | Missing provider cache accounting is represented as absent, not as a zero-token hit or miss. |
| Official DeepSeek path | The live probe forces the built-in official DeepSeek endpoint kind and base URL. |
| Remote cache isolation | Each arm uses a random non-identifying DeepSeek `user_id`. |
| Explicit cost gate | The example requires two affirmative CLI flags before network traffic. |
| Local-action isolation | Live probe runtimes expose no tools and execute no hooks or plugins. |
| Bounded experiment | Pairs, turns, and maximum output tokens have hard upper limits. |

## Validation Matrix

| Check | Method | Expected |
|---|---|---|
| Streaming usage | Request-body unit test | `include_usage` is set for verified DeepSeek and official OpenAI chat routes. |
| Full task bill | Ledger, SDK/HTTP fixture, child and compaction tests | Retries, summaries, late children and rebuilds remain charged; absent receipts stay incomplete. |
| Explicit checkpoint gating | Production builder and Converse HTTP fixture | Correct static/advancing blocks, model-specific TTL, no cross-provider fields. |
| Stable tool experiment | Mode-boundary regression | Review writes and mode escalation are rejected despite visible schemas. |
| Canonical fingerprint | Hash unit test with reordered JSON keys | Equivalent request bodies produce identical hashes. |
| Report privacy | Serialize a fingerprint built from sentinel private strings | No sentinel appears in output. |
| Arm perturbation | Fake-backend test | Stable system hash is unchanged; busted system hash changes. |
| Warm-up exclusion | Summary unit test | Only post-warmup cache usage affects the comparison. |
| Embedded API | Mock embedded-runtime integration test | Query returns one structured model-turn report. |
| Offline default | Build and invoke the example without both live flags | No provider call occurs. |
| Quality gates | `cargo fmt`, focused tests, `cargo check`, Clippy | No new formatting, compile, test, or lint failures. |

## Open Risks

- Provider eviction, load, and opaque cache partitioning can make a correctly
  constructed experiment inconclusive.
- Normal HTTP and empty-stream retries can add provider attempts beyond the
  reported logical request count; the per-attempt output bound still applies.
- Assistant responses are provider-generated, so later suffixes may differ
  across arms even though the scripted user turns match.
- The model-visible experiment marker makes the invalidation mechanism explicit
  but adds a small fixed token cost to both arms.
- Disabling tools isolates the cache measurement from local side effects, so a
  separate production query report is still required to study tool-schema
  churn.
- Fingerprints from different hash scopes are intentionally incomparable; the
  salt prevents report artifacts from becoming stable cross-runtime content
  identifiers.
- A checked-in harness does not constitute live cache evidence. A credentialed
  run and artifact review remain operational follow-up work.

## Source Journals

- [2026-08-21 DeepSeek cache probe](../journal/2026-08-21-deepseek-cache-probe.md)
- [2026-08-21 DeepSeek prefix cache locality](../journal/2026-08-21-deepseek-prefix-cache-locality.md)
- [2026-09-11 Prefix cache optimization](../journal/2026-09-11-prefix-cache-optimization.md)

## References

- [DeepSeek context caching](https://api-docs.deepseek.com/guides/kv_cache)
- [DeepSeek chat completions](https://api-docs.deepseek.com/api/create-chat-completion/)
- [Anthropic cost optimization cookbook](https://github.com/anthropics/claude-cookbooks/blob/main/cost_optimization/cost_optimization.ipynb)
- [Anthropic prompt caching](https://platform.claude.com/docs/en/build-with-claude/prompt-caching)
- [OpenRouter prompt caching](https://openrouter.ai/docs/guides/best-practices/prompt-caching)
- [OpenRouter tool-message content schema](https://github.com/OpenRouterTeam/typescript-sdk/blob/main/src/models/chattoolmessage.ts)
- [OpenRouter text-block cache controls](https://github.com/OpenRouterTeam/typescript-sdk/blob/main/src/models/chatcontenttext.ts)
- [OpenAI prompt caching](https://developers.openai.com/api/docs/guides/prompt-caching)
- [Bedrock prompt caching](https://docs.aws.amazon.com/bedrock/latest/userguide/prompt-caching.html)
