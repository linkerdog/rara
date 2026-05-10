# Subagent Enhancement & Auxiliary-Model Compression Spec

## Summary

This specification covers two improvements informed by review of Claude Code,
Codex, and OpenCode subagent implementations:

1. **Subagent enhancement**: add progress tracking, token metrics, activity
   descriptions, and background execution to RARA's `SpawnAgent`.
2. **Auxiliary-model retrieval compression**: use a cheap model to compress
   retrieval candidates before injecting them into the main model context.

Neither Claude Code, Codex, nor OpenCode implements aux-model retrieval
compression — this is a RARA-specific design.  Claude Code uses subagents
for task decomposition (the reference implementation), OpenCode has its own
TaskTool subagent system, and Codex has no subagent support (only a config
migration tool).

## Reference Systems

### Claude Code (`LocalAgentTask.tsx`)

- `ProgressTracker`: `{ toolUseCount, latestInputTokens, cumulativeOutputTokens, recentActivities }`
- `updateProgressFromMessage()`: called per turn to accumulate progress.
- `LocalAgentTaskState`: `{ agentId, prompt, model, progress, messages, isBackgrounded, pendingMessages }`
- Activity descriptions pre-computed from tool `getActivityDescription()`.
- Tasks can be backgrounded, mid-turn messages queued via `pendingMessages`.

### Codex (`external-agent-migration/`)

* Not a subagent system.  `external-agent-migration` is a **configuration
  migration tool** that reads Claude Code config files (`.mcp.json`, hooks,
  agents) and converts them to Codex's native format.  It does not spawn,
  track, or manage subagents.

### OpenCode (`tool/task.ts`)

TaskTool supports full subagent delegation:

- `subagent_type`: specialized agents with different permission sets.
- `task_id`: optional resume of a previous subagent session.
- Permission scoping: subagents can deny `todowrite` and `task` tools.
- `experimental.primary_tools`: configurable tool allowlists.
- `ctx.ask()`: interactive approval before spawning.
- `ctx.abort`: abort controller for cancellation.
- Output: structured with `task_id` + `<task_result>` wrapper.

## Part 1: Subagent Enhancement

### Current State (RARA)

`src/tools/agent.rs` `SpawnAgent` has:
- `AgentStatus`: `Spawning`, `Running`, `Completed`
- `created_at`: `Instant`
- `last_output`: `Vec<ContentBlock>`

Missing:
- Token usage per subagent
- Per-tool activity tracking (bash, file read, search)
- Background execution
- Mid-turn message queuing
- Shareable output data (`SharedData` in Claude Code)

### Design

#### Progress Tracker

```rust
pub struct SubagentProgress {
    pub tool_use_count: u32,
    pub total_input_tokens: u32,
    pub total_output_tokens: u32,
    pub recent_activities: Vec<SubagentActivity>,
}

pub struct SubagentActivity {
    pub tool_name: String,
    pub activity_description: String,
}
```

Update `SpawnAgent`:

```rust
pub struct SpawnAgent {
    pub status: AgentStatus,
    pub created_at: Instant,
    pub last_output: Vec<ContentBlock>,
    pub progress: SubagentProgress,           // NEW
    pub pending_messages: Vec<String>,         // NEW
    pub abort_token: Option<CancellationToken>, // NEW
}
```

#### Integration with RARA's existing AgentTool

RARA already has `src/tools/agent.rs` with `AgentTool::call()`. The
enhancements wire into the existing `spawn_agent` path:

1. `spawn_agent(subagent_id, instruction)` creates `SpawnAgent` with
   `SubagentProgress::default()` and registers it in `self.spawning_agents`.
2. Each subagent turn calls `update_progress_from_turn()` accumulating
   tool_use_count and tokens.
3. Activity descriptions use existing tool output (e.g. "bash: cargo check").
4. `pending_messages` drained via `send_message_to_subagent()` (future).

#### Display

In the TUI, subagent progress shows as:

```
  explore_agent: analyzing crates/               [4 tools · 12.3K tokens]
    Reading crates/rara-tools/src/tool.rs
    Running bash: cargo check
```

### Non-Goals for Part 1

- Full Claude Code-compatible `SharedData` output format.
- Subagent streaming to TUI (uses existing `AgentEvent` subscribers).
- Subagent tool restrictions or permission scoping.

---

## Part 2: Auxiliary-Model Retrieval Compression

### Background

Neither Claude Code, Codex, nor OpenCode implements aux-model compression.
This is a RARA-specific optimization for local-model use cases where the
main model has limited context or high cost.

### Design

The compression hook fits into the existing retrieval pipeline:

```
retrieval_candidates (from vectordb + file search + hooks)
    │
    ▼
estimate tokens (using `estimate_text_tokens()`)
    │  > COMPRESSION_THRESHOLD (2000 tokens)?
    │
    Yes → call aux model with system prompt
    │      "Compress these retrieval results into concise notes"
    │
    ▼
compressed_summary replaces raw candidates in context assembly
    │
    ▼
context assembly (uses compressed summary)
```

### System Prompt

```
You are a context compressor. Given retrieval results, output a
concise structured summary.

Include:
- File paths with one-line descriptions
- Memory records with relevance scores
- Tool output summaries (omit verbose logs)

Omit redundant or near-duplicate information.

Output format:
## Retrieved Context
### Files  
- path: summary
### Memory
- [score] key point
```

### Aux-Model Configuration

Already exists via `ProviderConfigState.auxiliary_model` which
resolves through the provider surface. The `RalphAgent` already has
`aux_model_completion()` using `self.llm_backend.classify()`.

### Integration Point

In `RalphAgent.do_start_of_turn_prep()` (after `refresh_*_candidates`):

```rust
if self.config.compression.enabled && !self.compressed_retrieval_text.is_empty() {
    self.compress_retrieval_candidates().await;
}
```

### Caching

Store compressed result sharded by the hash of raw retrieval candidates.
If the same candidates are retrieved across turns, reuse the cached
compressed version.

Cache lives in agent memory (not persisted to disk).

### Display

In the context budget display:

```
Memory            2.3K  (1.2%)   [compressed 0.8K]
```

`[compressed N]` tag replaces or supplements the `[cached]` tag when
aux-model compression was applied.

---

## Implementation Plan

### Subagent Enhancement

1. Add `SubagentProgress` and `SubagentActivity` structs.
2. Extend `SpawnAgent` with progress, pending_messages, abort_token.
3. Add `update_progress_from_turn()` to accumulate per-turn metrics.
4. Wire progress into existing `sub_agent_display.rs` rendering.
5. Add snapshot tests for progress display.

### Aux-Model Compression

1. Add `AuxCompressionConfig { enabled: bool, threshold_tokens: usize }`.
2. Add `compress_retrieval_candidates()` function in agent loop.
3. Wire compressed result into `RuntimeContextInputs`.
4. Add `[compressed N]` tag to `/context` display.
5. Cache compressed result per candidate-set hash.

---

## Verification

### Subagent Enhancement

- Unit test: `SubagentProgress` accumulation.
- TUI snapshot: subagent with progress bar and activity list.
- Manual: `spawn_agent` a task, verify progress updates in sidebar.

### Aux-Model Compression

- Unit test: compression prompt generation.
- Unit test: candidate set hashing for cache.
- Integration: run with aux model configured, verify `[compressed N]` tag.
