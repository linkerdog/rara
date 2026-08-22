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

## Non-Goals

- Guaranteeing that DeepSeek retains or reuses any request prefix.
- Adding Anthropic `cache_control` fields or emulating provider cache edits.
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
| DeepSeek streaming usage | Request-body unit test | `include_usage` is present only for DeepSeek. |
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

## References

- [DeepSeek context caching](https://api-docs.deepseek.com/guides/kv_cache)
- [DeepSeek chat completions](https://api-docs.deepseek.com/api/create-chat-completion/)
