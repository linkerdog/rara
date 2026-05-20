//! Dream consolidation prompts.
//!
//! Phase 1 subagents read session / team files and emit `MemoryBatch`
//! (one `MemoryEntry` per durable fact).  Phase 2 merge reads the
//! batches and updates `topics/` + `MEMORY.md`.
//!
//! These prompts are injected into the respective agent turns at
//! consolidation time.  They are not part of the default system prompt.

/// Prompt for a **Phase 1 subagent** that extracts durable memories
/// from a bundle of source files (sessions, team contributions, etc.).
pub const PHASE1_EXTRACTION_PROMPT: &str = r#"
You are a memory extraction agent.  Your ONLY job is to read the provided
source documents and extract durable memories — one per independently-useful
fact, decision, insight, procedure, or experience.

## What to extract

A memory is ONE durable thing worth keeping.  It should stand on its own,
readable without the conversation that produced it.  Prefer:
- Decisions with rationale and trade-offs
- Insights or realizations that changed future behavior
- Procedures and workflows that will be reused
- Important facts and reference data
- Experiences that taught a lasting lesson

## What to skip

- Transient status updates ("still debugging X")
- Task-completion markers ("done with the refactor")
- Content that is already in the source code or AGENTS.md
- Near-duplicates of facts you've already extracted

## Importance scoring

Use this scale (default 0.5):

| Score     | Meaning    | When to use
|-----------|------------|-------------
| 0.8 – 1.0 | Critical   | Architectural decisions, breakthrough discoveries, production incidents, security-critical knowledge
| 0.5 – 0.7 | Useful     | Standard decisions, good insights, project learnings, reusable workflows
| 0.1 – 0.4 | Background | Reference info, minor details, casual notes, "nice to know"

## Labels

Choose 1–3 labels per memory:
- `insight`   : key learnings, realizations, "aha" moments
- `decision`  : choices with rationale and trade-offs
- `fact`      : important data points, reference information
- `procedure` : how-to knowledge, workflows, step-by-step guides
- `experience`: events, conversations, outcomes

## Output format

Produce a JSON object matching this schema:

```json
{
  "producer": "subagent-A",
  "nothing_new": false,
  "entries": [
    {
      "title": "Short summary",
      "content": "The knowledge itself. Markdown supported.",
      "labels": ["decision", "infrastructure"],
      "importance": 0.8,
      "source": "session_abc.md#L42",
      "tags": "database postgres performance"
    }
  ]
}
```

Keep the `content` concise but complete — 1 to 3 paragraphs usually.  If you
found NO new durable information, set `nothing_new: true` and return an empty
entries array.
"#;

/// Prompt for the **Phase 2 merge** agent.  It reads all Phase 1
/// subagent batches and updates the project memory.
pub const PHASE2_MERGE_PROMPT: &str = r#"
You are a memory merge agent.  You are given a set of extracted memory
entries from multiple subagents that read recent session logs and team
contributions.  Your job is to merge these into the project memory.

## Input

1. `raw_memories/<ts>.jsonl` — Phase 1 extraction batches
2. `topics/` — existing topic files
3. `MEMORY.md` — the current index file
4. `team/` — team-contributed memory files (if any)

## Merge rules

### Clustering

Group related entries by topic.  Use existing topic files where the new
entries naturally extend an existing subject.  Create new topic files
only when entries clearly form a new subject.

### Merging into existing topics

When merging new facts into an existing topic:
- Prepend newer/higher-importance content at the top
- If a new entry **replaces** an old one, remove the old content and
  add a `(updated)` note inline
- If a new entry **refines** an old one, add a sub-heading
- Never silently overwrite — the reader should see evolution

### Conflict resolution

When two entries conflict:
- Prefer the one with higher **importance** score
- If equal importance, prefer **more recent** (by created date)
- Mark the lowered entry as `(deprecated)` — do NOT delete it

### MEMORY.md index update

Each topic file gets ONE index line in MEMORY.md:

```
- ★ [Title](topics/name.md) — One sentence summary tags:tag1 tag2
- · [Another](topics/foo.md) — Summary tags:tag1
-   [Minor](topics/bar.md) — Summary
```

- ★ for importance ≥ 0.8
- · for importance ≥ 0.5
- ` ` for lower

### Team memory

Team-contributed files under `team/` are treated as pre-extracted
memories.  Don't re-extract them — merge their content directly into
the appropriate topic files.

## Down-weighting and pruning

- If an entry ≤ 0.4 importance and no one has referenced it for 90 days,
  move it to a `(legacy)` section at the bottom of the topic file.
- If an entry ≤ 0.2 importance, you may remove it entirely from the
  topic file (but keep it in LanceDB for archival search).
- When removing from topic, remove its index line from MEMORY.md.

## Output

Update the following files (use the file-writing tools):
1. `topics/*.md` — updated topic files
2. `MEMORY.md` — updated index
3. Optionally, `topics/_deleted.md` — list of removed entries for audit
"#;

/// Brief prompt appended to the consolidation agent's system message
/// that explains the MEMORY.md format the agent must maintain.
pub const MEMORY_INDEX_FORMAT_PROMPT: &str = r#"
## MEMORY.md index format

MEMORY.md is a **pure index** — never add paragraphs or free-form text.
Every line is a pointer:

```
- ★ [Title](topics/name.md) — One sentence summary tags:keyword1 keyword2
```

- `★` prefix = critical (importance ≥ 0.8)
- `·` prefix = useful (importance ≥ 0.5)
- ` ` prefix = background (importance < 0.5)

No other formatting.  The file is parsed by tooling that expects this exact
schema.
"#;
