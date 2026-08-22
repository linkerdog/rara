# Project Context Merge

## Problem

RARA needs one deterministic system-prompt prefix for durable repository
instructions while keeping per-turn state close to the user request. Mixing
environment, execution mode, live protocol sources, or retrieved memory into
the system message invalidates automatic prefix caches before reusable history
can be reached.

## Scope

- Merge file-based project instructions and stable workspace memory into one
  `project_context` system section.
- Keep stable skill metadata and language guidance in the system prompt.
- Route environment, execution mode, protocol/LSP sources, and retrieved memory
  through typed model context on the current user message.
- Preserve typed model context in model-visible history so later requests keep
  the exact bytes that earlier requests sent.

## Non-Goals

- Changing how `ProjectInstruction`, `UserInstruction`, or `LocalMemory`
  sources are discovered.
- Treating query-dependent retrieval as stable workspace memory.
- Adding provider-specific cache keys or Anthropic `cache_control` fields.
- Guaranteeing a provider cache hit; providers may evict or decline an
  otherwise reusable prefix.

## Architecture

The request is assembled in this order:

```text
deterministically ordered tool schemas and stable system prompt
  base prompt
  project_context
    project instructions
    stable workspace memory
  skills
  language best practices
  append prompt and session capability policy
append-only conversation history
current user message
  changed environment context, if any
  changed execution-mode context, if any
  changed protocol/LSP context, if any
  selected retrieved memory, if any
  human-authored request text
```

`rara_model_context` is the typed carrier for the current-user attachments:

```json
{
  "type": "rara_model_context",
  "kind": "environment",
  "text": "<environment_context>...</environment_context>"
}
```

The carrier is persisted with the user message. Transcript rendering, memory
retrieval queries, and memory-lifecycle capture ignore the carrier, while model
provider serializers render its `text` field. This makes the model-visible
history append-only without presenting runtime attachments as user-authored
text.

### Project context rendering

`project_context` contains up to two labeled subsections:

- `### Project Instructions` contains ordered user and project instruction
  sources.
- `### Session Memory` contains stable local workspace memory.

The complete section is omitted when both source classes are absent.

### Turn-context deltas

Environment, execution mode, and protocol prompt sources are appended only
when their rendered value differs from the latest value of the same kind in
history. Retrieved memory is query-dependent and is attached to every turn
where selection returns content. When protocol sources disappear, the runtime
appends an explicit cleared-state context so stale instructions do not remain
authoritative.

### Prefix stability

The literal `__DYNAMIC_BOUNDARY__` marker is not a provider cache primitive and
must not appear in requests. RARA sends a plain system string. Stable-source
changes may intentionally produce a new system prefix, but ordinary changes to
cwd, branch, mode, diagnostics, protocol sources, or retrieval do not rewrite
the earlier prefix.

## Contracts

| Contract | Detail |
|---|---|
| Stable system | Environment, mode, protocol/LSP data, and retrieved memory are absent from the system message. |
| Merged project context | File instructions and stable workspace memory share the `project_context` section. |
| Append-only model view | A later request preserves all earlier model-visible messages byte-for-byte until durable compaction. |
| Hidden runtime attachments | `rara_model_context` is rendered to providers but omitted from human transcript and memory-query text. |
| Cleared protocol state | Removal of previously active protocol sources is represented by a new turn-context delta. |
| Provider neutrality | OpenAI-compatible, Codex Responses, Ollama, Gemini, Bedrock, and local prompt serializers preserve model-context text. |

## Validation Matrix

| Check | Method | Expected |
|---|---|---|
| Stable project merge | `rara-instructions` prompt tests | Project instructions and memory remain in `project_context`. |
| Dynamic placement | `rara-instructions` turn-context tests | Environment, mode, and protocol sources are absent from system and present in turn context. |
| Prefix invariant | Agent two-query regression | Request two starts with every message from request one. |
| Memory placement | Agent retrieval regression | Selected memory is a persisted `rara_model_context` block before user text. |
| Provider serialization | DeepSeek, Codex Responses, Gemini, Bedrock, and local-renderer regressions | Context text is model-visible; DeepSeek sends no fake boundary or `cache_control`. |
| Quality gates | `cargo fmt`, focused tests, `cargo check`, Clippy | No new formatting, compile, or lint failures. |

## Operational Notes

- Shipping this layout causes a one-time cold prefix for existing sessions.
- DeepSeek cache effectiveness must be measured from official
  `prompt_cache_hit_tokens` and `prompt_cache_miss_tokens`; request shape alone
  is not proof of a hit.
- Mode-specific tool availability remains a separate request field and can
  intentionally create a new provider prefix when the mode changes.

## Open Risks

- Live edits to stable project instructions still change the system prompt and
  intentionally invalidate the provider prefix.
- A future protocol source with stronger authority requirements may need a
  dedicated signed context class rather than generic user-role placement.

## Source Journals

- [2026-08-21 DeepSeek prefix cache locality](../journal/2026-08-21-deepseek-prefix-cache-locality.md)
- [Provider cache observability](provider-cache-observability.md)
