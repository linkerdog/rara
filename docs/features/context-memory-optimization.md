# Context & Memory Optimization

## Summary

After each turn, RARA currently performs three full refreshes that are
unnecessary when the workspace hasn't changed:

1. `refresh_file_search_candidates()` — rescans the entire workspace.
2. `refresh_memory_retrieval_candidates()` — requeries the vector database.
3. `clear_completed_interactions()` — drops completed tool results from the
   active turn (correct behavior, but the display should show them as
   persistent for the user).

The goal is to replace full rescans with **incremental caching** so that repeat
turns in the same workspace avoid redundant I/O, and to give the user a
**durable summary** of completed tool calls across turns.

Claude Code persists task lists to `~/.claude/tasks/` and retains full
tool results in the conversation transcript.  RARA should adopt a similar
model for todo persistence and tool visibility, and add file-search
caching as a RARA-specific optimization — since RARA runs a local vector
database, each retrieval costs more than a hosted backend call.

## Goals

1. Track which tool results are already consumed so `clear_completed_interactions`
   does not silently erase the user's visible history.
2. Increment the file-search cache on filesystem changes instead of rebuilding
   from scratch each turn.
3. Cache the previous turn's memory retrieval set. Only refresh when the user
   query semantic distance exceeds a threshold.
4. Improve token estimation from `chars / 3` to a tiered heuristic calibrated
   against real backend tokenizer output.
5. Refine the context budget display in the TUI sidebar and `/context` overlay
   to show:
   - per-section token usage with % of window
   - which sections are cached vs freshly computed
   - total budget bar capped at 100%.

## Non-Goals

- Rewriting the context assembler object model (covered in `context-architecture.md`).
- Changing the compaction lifecycle.
- Adding a new persistence schema for file indexes.

---

## Problem Statement

### File search rescans every turn

`refresh_file_search_candidates` in `src/agent/memory_retrieval.rs:20` triggers
a full `file_search` scan (`retrieval_candidates(&query, 64)`) each turn.  For
a 10k-file repository this can take hundreds of milliseconds and produces an
identical result across consecutive turns unless the user modifies files between
turns.

### Memory retrieval re-queries every turn

`refresh_memory_retrieval_candidates` in `src/agent/memory_retrieval.rs:10`
runs a full LanceDB hybrid search each turn.  Even when the user query is
almost identical to the previous turn, the same vectors are computed and the
same results retrieved.

### Token estimation is crude

`estimate_text_tokens` in `src/context/assembler.rs:371` divides character
count by 3.  This underestimates for code-heavy prompts (many short tokens)
and overestimates for prose (fewer tokens per character).  A tiered heuristic
calibrated against a tokenizer is needed.

### Context display is unclear

The sidebar and `/context` overlay show a budget breakdown but don't indicate
which sections are stale (recomputed) vs cached.  The total usage bar was
misleading (showed 1860% when budget fields used char counts vs token counts).
It's now capped at >100% but should also show per-section staleness.

---

## Design

### 1. File search: turn-granularity cache

Add a `FileSearchCache` to `RalphAgent`:

```rust
struct FileSearchCache {
    /// Result from the last full scan.
    candidates: Vec<RetrievalCandidate>,
    /// mtime of the newest workspace file at scan time.
    newest_mtime: Option<SystemTime>,
}
```

**Refresh policy**:
- First turn: always full scan.
- Subsequent turns: stat the workspace root.  If no file is newer than
  `newest_mtime`, reuse the cached candidates.
- If any file is newer, re-scan.

**Lifespan**: cache lives for the duration of the agent session.

### 2. Memory retrieval: semantic-distance throttle

Add a `MemoryRetrievalCache`:

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
- Otherwise, re-query.

**Token saving**: avoiding a full retrieval saves the LLM from re-reading the
same memory snippets.

### 3. Token estimation: tiered heuristic

Replace `chars / 3` with a tiered table calibrated against a tokenizer
(tiktoken `cl100k_base`):

| Input type | Token factor | Rationale |
|-----------|-------------|-----------|
| General prose | chars / 3.5 | English prose averages ~3.5 chars/token |
| Code blocks | chars / 2.5 | Code has shorter tokens (operators, keywords) |
| JSON / structured | chars / 2.0 | Punctuation-heavy, many single-char tokens |
| Unknown | chars / 3.0 | Fallback |

`estimate_text_tokens` should accept an optional `ContentKind` parameter and
apply the appropriate factor.  `ContentKind::Unknown` uses the fallback.

### 4. Completed interactions: persistent commit log

Replace `clear_completed_interactions` with a **committed interaction log**:

- When a tool call completes, move it from `active_turn.interactions` to
  `committed_interactions`.
- Render committed interactions in the TUI with a dimmed style (like
  `[done] bash: cargo check · 0.3s`).
- The summary line at the bottom of the action pane shows:
  `[3 tools completed this turn]`

This gives the user a **visible audit trail** of what the agent did, even
after those results have been consumed.

---

## TUI Display Specification

### Context sidebar (≥120 cols) and `/context` overlay

```
  22.3% used                                          ← percentage with bar
  ── Budget Breakdown ──
  System prompt    18.2K  ( 9.1%)                     ← per-section %
  Workspace         4.8K  ( 2.4%)   [cached]          ← staleness
  Active turn      12.1K  ( 6.0%)
  History          38.0K  (19.0%)
  Memory            2.3K  ( 1.2%)   [cached]
  Retrieval        15.0K  ( 7.5%)   [cached]
  Output            8.0K  ( 4.0%)
  Free            100.6K  (50.3%)
  ─────────────────────────────────
  Window          200.0K tokens                        ← model context limit
```

**Key changes**:
1. Percentage-only summary (already implemented — capped at `>100%`).
2. `[cached]` tag next to sections served from turn-granularity cache.
3. Total window size shown at bottom.
4. Each section shows both absolute token count and % of window.

### Completed tool summary (bottom of action pane)

```
  [✓] cargo check            · 2.3s
  [✓] cargo fmt              · 0.1s
  [✓] git push               · 1.2s
  ──────────────────────────
  3 tools completed this turn
```

---

## Implementation Plan

### Phase 1: Display (already in progress)

- [x] Cap aggregate percentage at `>100%`.
- [ ] Add `[cached]` tag to budget rows for in-memory sections.
- [ ] Add completed tool summary to bottom pane footer.

### Phase 2: File search cache

- [ ] Add `FileSearchCache` struct to `RalphAgent`.
- [ ] Implement `newest_mtime` check in `refresh_file_search_candidates`.
- [ ] Add cache invalidation on explicit file writes.

### Phase 3: Memory retrieval throttle

- [ ] Add `MemoryRetrievalCache` struct.
- [ ] Compute cosine similarity threshold.
- [ ] Fall back to query on cache miss.

### Phase 4: Token estimation improvement

- [ ] Add `ContentKind` enum.
- [ ] Implement tiered heuristic in `estimate_text_tokens`.
- [ ] Annotate content sections with `ContentKind` in the assembler.

---

## Verification

- Unit tests for `FileSearchCache` hit and miss.
- Unit tests for `MemoryRetrievalCache` similarity threshold.
- Unit tests for `estimate_text_tokens` with known inputs.
- TUI snapshot tests for `[cached]` tags and completed tool summary.
- Manual: open a workspace, run two turns, verify file search is not
  re-executed on second turn.
