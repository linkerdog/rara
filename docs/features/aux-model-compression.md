# Auxiliary-Model Compression Hook

## Summary

Add an optional aux-model compression step between retrieval and context assembly.
When retrieval candidates exceed a token budget, a smaller/cheaper auxiliary model
compresses them into concise structured summaries before they are injected into
the main model's context window.

The hook operates on retrieval candidates only — it does **not** modify persistent
memory records, and it does **not** change the compaction pipeline.

## Motivation

Retrieval candidates can be verbose:
- file search results list many paths and snippets
- memory records contain full content blobs
- hook output may include large text blocks

When the main model's context window is expensive (GPT-4, Claude Opus), compressing
these candidates with a cheap flash model can save substantial token costs without
degrading quality.

## Design

### Trigger conditions

The hook fires when **both** conditions are met:

1. A configured auxiliary model is available (provider + model name).
2. The total estimated token count of retrieval candidates exceeds a configurable
   threshold (default: 2000 tokens).

### Processing pipeline

```
retrieval candidates
    │ (estimated tokens > threshold?)
    │ Yes: send to aux model with compression prompt
    ↓
aux model compresses into structured markdown
    │
    ↓
compressed summary replaces raw candidates in context
    │ (otherwise: raw candidates pass through unchanged)
    ↓
context assembly
```

### Compression prompt

The aux model receives a fixed system prompt:

```
You are a context compressor. Summarize the following retrieval
candidates into concise structured notes. Preserve:

1. File paths and what's in them
2. Memory records with relevance scores
3. Code snippets (keep if small, note if large)
4. Any specific versions, commit SHAs, or dates

Omit:
- Redundant or duplicate information
- Irrelevant general knowledge
- Full source code (note "see file X" instead)

Output format (markdown):
## Retrieved Context
### Files
- path: summary
### Memory
- [score] key point
### Other
- source: note
```

### Compression output format

| Section | Content |
|---------|---------|
| `Files` | Path + one-line summary per file |
| `Memory` | Key memory point with relevance score |
| `Other` | Any other retrieval source with note |

### Caching

Compressed output is cached per retrieval candidate set hash. If the same
candidates are retrieved again (e.g. between consecutive turns with similar
queries), the cached compression is reused instead of re-calling the aux model.

### Token estimation

Before compression, estimate the total token count of raw candidates using
the tiered heuristic from `context-memory-optimization.md`. After compression,
record the compressed token count for observability.

### Observability

The context budget display shows:

```
Memory            2.3K  ( 1.2%)   [compressed 0.8K]
```

`[compressed N]` tag indicates aux-model compression was applied, with
the post-compression token count.

## Configuration

In `config.toml`:

```toml
[aux_model]
provider = "openai"
model = "gpt-4o-mini"
enabled = true
compression_threshold_tokens = 2000
```

Environment overrides:
- `RARA_AUX_MODEL_PROVIDER`
- `RARA_AUX_MODEL`
- `RARA_AUX_COMPRESSION_THRESHOLD`

## Non-Goals

- Compression of the main conversation history (handled by compaction).
- Compression of the system prompt.
- Multi-step compression pipelines.
- Vector-based candidate reranking.

## Implementation Plan

### Phase 1: Scaffold

1. Add `AuxModelConfig` to `rara-config`.
2. Add `compress_retrieval_candidates()` function to context assembler.
3. Wire up in `assemble()` — call after retrieval, before budget allocation.
4. Add `[compressed N]` display in context budget.

### Phase 2: Cache

1. Hash retrieval candidate set.
2. Store compressed output in `MemoryRetrievalCache`.
3. Invalidate cache when candidates change.

### Phase 3: Token estimation

1. Use tiered heuristic to estimate pre-compression tokens.
2. Count post-compression tokens.
3. Expose both in runtime snapshot for `/context` display.

## Verification

- Unit test: compression prompt generation.
- Unit test: candidate set hashing and cache hit/miss.
- Integration test: run with aux model configured, verify compressed output
  replaces raw candidates.
- Manual: observe `[compressed N]` tag in context display.
