# Subagent Enhancement & Auxiliary-Model Compression Spec

## Summary

This specification covers two improvements informed by review of Claude Code,
Codex, and OpenCode subagent implementations:

1. **Subagent enhancement**: add progress tracking, token metrics, activity
   descriptions, and background execution to RARA's subagent system.
2. **Auxiliary-model retrieval compression**: use a cheap model to compress
   retrieval candidates before injecting them into the main model context.

Neither Claude Code, Codex, nor OpenCode implements aux-model retrieval
compression — this is a RARA-specific design. Claude Code, Codex, and OpenCode
all expose structured subagent lifecycle state rather than deriving it from
ordinary interaction prompts or tool-result text.

## Reference Systems

### Claude Code (`LocalAgentTask.tsx`, `CoordinatorAgentStatus.tsx`)

- `ProgressTracker`: `{ toolUseCount, latestInputTokens, cumulativeOutputTokens, recentActivities }`
- `updateProgressFromMessage()`: called per turn to accumulate progress.
- `LocalAgentTaskState`: `{ agentId, prompt, model, progress, messages, isBackgrounded, pendingMessages }`
- Activity descriptions pre-computed from tool `getActivityDescription()`.
- Tasks can be backgrounded, mid-turn messages queued via `pendingMessages`.

### Codex (`tui/src/app/agent_status_feed.rs`, `tui/src/multi_agents.rs`)

- Agent status is projected from typed child-thread lifecycle events.
- `/agent` reports active and recently completed agents by structured status.
- Activity previews are bounded by item count, line count, and grapheme count.
- Raw reasoning deltas are excluded from agent activity previews.

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

`AgentTreeControl` already owns session-scoped child records, concurrency,
cancellation, mailboxes, background execution, resolved provider/model data,
and final token totals. `SubagentProgress` already defines bounded recent
activity and live token/tool counters.

The missing integration is projection and display:

- subagent execution does not feed its typed `AgentEvent` stream into
  `SubagentProgress` while it is running;
- `RuntimeSnapshot` does not contain child-agent lifecycle state;
- the sidebar incorrectly labels pending approval/input interactions as
  subagents, so the displayed identities do not represent the agent tree;
- `/status` reports configured agent definitions but not live or recently
  completed child agents.

### Design

#### Progress Tracker

```rust
pub struct SubagentProgress {
    pub tool_use_count: usize,
    pub tool_use_total: Option<usize>,
    pub total_input_tokens: usize,
    pub total_output_tokens: usize,
    pub activity: Vec<String>,
}
```

The subagent query reporter updates this state from typed events:

- `ToolUse` increments the tool count and records a bounded tool label;
- `Status` records a sanitized, bounded activity label;
- `ModelRequest` and `ModelResponse` accumulate reported token counts;
- assistant text and reasoning deltas are not copied into the progress view;
- final completion metrics replace provisional live counters.

#### Runtime Projection

The TUI reads a bounded snapshot of the root's full descendant tree from the
session-owned `AgentTreeControl`.
The runtime client retains both the tree handle and root session identity even
while the root `Agent` is moved into an asynchronous task. The existing TUI
tick refreshes this projection and requests a redraw only when it changes.

Presentation state contains values only. It must not own `AgentTreeControl`,
task handles, cancellation tokens, mailboxes, or other mutable runtime
behavior.

#### Display

The wide sidebar and `/status` overview show the same structured projection:

```
  [>] explore_agent (explore)
      4 tools · 12.3k tokens · Using read_file
```

Running agents sort before terminal agents. The sidebar displays at most five
records and reports how many additional records are hidden. Status markers and
semantic colors distinguish running, completed, failed, and cancelled agents.

### Non-Goals for Part 1

- Full Claude Code-compatible `SharedData` output format.
- Raw subagent assistant or reasoning streaming in the parent transcript.
- New subagent control actions in the TUI.
- Changes to subagent tool restrictions or permission scoping.

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

In `Agent.do_start_of_turn_prep()` (after `refresh_*_candidates`):

```rust
if retrieval_token_count > self.config.compression_threshold_tokens {
    self.compress_retrieval_candidates().await;
}
```

### Caching

Store compressed result keyed by the hash of raw retrieval candidates.
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

1. Feed typed child events into the existing `SubagentProgress` record.
2. Add a bounded child-agent activity projection to `AgentTreeControl`.
3. Retain the session tree handle in `RuntimeClient` while the root agent runs.
4. Refresh projection state on the existing TUI tick.
5. Render the projection in the sidebar and `/status` overview.
6. Add focused progress, state projection, and rendering tests.

### Aux-Model Compression

1. Add `AuxCompressionConfig { enabled: bool, threshold_tokens: usize }`.
2. Add `compress_retrieval_candidates()` function in agent loop.
3. Wire compressed result into `RuntimeContextInputs`.
4. Add `[compressed N]` tag to `/context` display.
5. Cache compressed result per candidate-set hash.

---

## Verification

### Subagent Enhancement

- Unit test: typed events update bounded `SubagentProgress` state.
- Unit test: only children of the current root session are projected.
- Render test: pending approvals are not rendered as subagents.
- Render test: running and completed agent identities, status, tool count,
  tokens, and recent activity appear in the sidebar and `/status`.
- Manual: spawn parallel agents and verify progress changes without waiting for
  the root turn to finish.

### Aux-Model Compression

- Unit test: compression prompt generation.
- Unit test: candidate set hashing for cache.
- Integration: run with aux model configured, verify `[compressed N]` tag.
