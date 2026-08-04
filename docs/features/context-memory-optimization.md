# Context & Memory Optimization

## Summary

After each turn, RARA has two expensive operations that are often redundant:

1. `refresh_memory_retrieval_candidates()` — re-queries the vector database
   with a fresh embedding, even when the user query is nearly identical.
2. `clear_completed_interactions()` — drops tool results from the active turn
   so the user can't see what the agent already did.

Additionally, `estimate_text_tokens()` uses a crude `chars / 3` heuristic
that misestimates for code-heavy content.

The goal is to **avoid redundant embedding calls** and give the user a
**visible audit trail** of completed tool calls.

Claude Code persists task lists to `~/.claude/tasks/` and retains full
tool results in the conversation transcript.  RARA should adopt a similar
model for todo persistence and tool visibility, and add retrieval throttling
as a RARA-specific optimization — since RARA runs a local vector database,
each retrieval costs more than a hosted backend call.

## OpenCode TUI Boundary

OpenCode's TUI consumes a server-owned session projection. Messages and tool
parts have stable identities and explicit lifecycle states; compaction is a
first-class session item. RARA follows the same boundary incrementally:

- runtime events may carry an optional `call_id` and the TUI stores a typed
  tool payload alongside the legacy role/message representation;
- role/message remains a persistence compatibility format, not the source of
  runtime semantics;
- retrieval caching and compaction decisions remain runtime-owned;
- TUI rendering consumes snapshots and structured events and only presents
  status, summaries, and inspectable details.

The next implementation step is to complete the runtime session projection
for tool lifecycle and compaction events before adding retrieval-cache UI.

## Goals

1. Cache the previous turn's memory retrieval set. Only refresh when the
   user query semantic distance exceeds a threshold.
2. Track which tool results are already consumed so `clear_completed_interactions`
   does not silently erase the user's visible history.
3. Improve token estimation from `chars / 3` to a tiered heuristic calibrated
   against real backend tokenizer output.

## Non-Goals

- File search caching: file scanning is cheap (metadata-only) and not worth
  the complexity of mtime tracking.
- Rewriting the context assembler object model.
- Changing the compaction lifecycle.

---

## Problem Statement

### Memory retrieval re-queries every turn

`refresh_memory_retrieval_candidates` in `src/agent/memory_retrieval.rs:10`
runs a full LanceDB hybrid search each turn.  When the user query is
almost identical to the previous turn (e.g. "continue", "fix it"),
the same embedding is computed and the same results retrieved.  This is
wasteful: embedding API calls are the most expensive operation in the
retrieval pipeline.

### Completed interactions vanish

`clear_completed_interactions` in `src/agent.rs` removes completed tool
calls from the active turn.  The user sees tool progress in real time
but once the agent moves to the next response, those completed tools
disappear from the TUI.  There's no persistent summary of what happened.

### Token estimation is inaccurate

`estimate_text_tokens` in `src/context/assembler.rs:371` divides character
count by 3.  Code-heavy content has many short tokens (operators, keywords)
while prose has fewer.  The uniform factor leads to budget over-allocation
for code and under-allocation for prose.

---

## Design

### 1. Memory retrieval: semantic-distance throttle

Add a `MemoryRetrievalCache` to `RalphAgent`:

```rust
struct MemoryRetrievalCache {
    last_query: String,
    last_embedding: Option<Vec<f32>>,
    results: Vec<RetrievalCandidate>,
}
```

**Refresh policy**:
- First turn: always query.
- Subsequent turns: embed the new query, compute cosine similarity against
  `last_embedding`.  If similarity > 0.85, reuse cached results.
- Otherwise, re-query and update the cache.

**Token saving**: avoiding a full retrieval saves the LLM from re-reading
stale memory snippets that haven't changed.

### 2. Completed interactions: persistent commit log

Replace `clear_completed_interactions` with a **committed interaction log**:

- When a tool call completes, move it from `active_turn.interactions` to
  `committed_interactions`.
- Render committed interactions in the TUI with a dimmed checkmark style:
  `[done] bash · cargo check · 0.3s`
- The bottom pane footer shows a summary:
  `3 tools completed`

This gives the user a visible audit trail of what the agent already did,
even after those results have been consumed by the LLM.

### 3. Token estimation: tiered heuristic

Replace `chars / 3` with a tiered calibration:

```rust
fn estimate_text_tokens(text: &str, kind: ContentKind) -> usize {
    let factor = match kind {
        ContentKind::Prose => 3.5,
        ContentKind::Code  => 2.5,
        ContentKind::Json  => 2.0,
        ContentKind::Unknown => 3.0,
    };
    text.len().div_ceil(factor as usize)
}
```

Annotate content sections with `ContentKind` in the assembler so each
section uses its appropriate factor.

---

## TUI Display Specification

### Completed tool summary (bottom of action pane)

```
  [✓] cargo check            · 2.3s
  [✓] cargo fmt              · 0.1s
  [✓] git push               · 1.2s
  ──────────────────────────
  3 tools completed
```

### Context sidebar and `/context` overlay

```
  22.3% used
  ── Budget Breakdown ──
  System prompt    18.2K  ( 9.1%)
  Workspace         4.8K  ( 2.4%)
  Active turn      12.1K  ( 6.0%)
  History          38.0K  (19.0%)
  Memory            2.3K  ( 1.2%)   [cached]
  Output            8.0K  ( 4.0%)
  Free            100.6K  (50.3%)
```

- `[cached]` tag next to memory section when served from retrieval cache.
- Percentage capped at `>100%` when char-count budgets overflow.

---

## Implementation Plan

### Phase 0: Session projection

- [x] Preserve optional tool call IDs in structured runtime events.
- [x] Attach typed tool lifecycle payloads to TUI transcript entries while
      retaining legacy role/message persistence.
- [x] Add runtime-owned compaction events and typed session projection records.

Compaction records reuse the existing durable `PersistedCompactionEvent` path.
The runtime reporter publishes the same boundary as a structured session event,
and the TUI stores a typed compaction payload while retaining role/message
compatibility persistence.

### Phase 1: Display

- [ ] Add completed tool summary to bottom pane footer.
- [ ] Add `[cached]` tag to budget rows for memory section.

Compaction entries use a dedicated timeline cell with token savings and recent
file count. They are not rendered as generic assistant or tool messages.

### Phase 2: Memory retrieval throttle

- [ ] Add `MemoryRetrievalCache` struct to `RalphAgent`.
- [ ] Compute cosine similarity threshold.
- [ ] Fall back to query on cache miss.

### Phase 3: Token estimation

- [ ] Add `ContentKind` enum.
- [ ] Implement tiered heuristic in `estimate_text_tokens`.
- [ ] Annotate content sections in the assembler.

---

## Verification

- Unit test: `MemoryRetrievalCache` hit on similar query, miss on different.
- Unit test: `estimate_text_tokens` with prose, code, json inputs.
- TUI snapshot: completed tool summary rendering.
- Manual: run two turns with similar queries, verify second turn reuses
  cached memory (check logs for "cache hit").
