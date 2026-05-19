# Context Subsystem

## Overview

The context subsystem assembles the system prompt sent to the LLM each turn.
It combines project instructions (AGENTS.md), workspace memory, runtime config,
memory summaries, and hook prompts into an `EffectivePrompt`.

## Assembly Pipeline

```
ContextAssembler::assemble(mode)
    │
    ├── prompt::build_effective_prompt(workspace, runtime, mode)
    │       ├── workspace.project_memory_string()  — AGENTS.md etc.
    │       ├── workspace.available_skills()        — loaded skills
    │       ├── runtime.prompt_sources()            — MCP protocol, tool specs
    │       └── runtime.hooks_prompt()              — file-based hook content
    │
    ├── memory_files::read_memory_section()         — read-path template + summary
    │       ├── MEMORY_READ_PATH_HEADER             — usage instructions
    │       ├── read_summary_for_context()          — truncated summary.md
    │       └── MEMORY_READ_PATH_FOOTER
    │
    └── AssembledContext { effective_prompt, compact_instruction }
```

## Key Components

### ContextAssembler (`assembler.rs`)

The main entry point. Takes `&WorkspaceMemory`, `&PromptRuntimeConfig`, and
`PromptMode`. Returns `AssembledContext` with the fully assembled prompt text
and compact instruction.

Injections:
- `## Memory` section (read-path template + summary) when memory exists
- `## Hooks` section when hooks are discovered
- Both injected AFTER the base effective prompt, before sending to LLM

### Memory Selection (`memory_selection.rs`)

Determines which memory sources are available and selects candidates for
injection into context. Key functions:

- `available_memory_sources()` — what's available (workspace, session, global)
- `retrieval_candidates()` — selects top-k candidates based on similarity
- Scoring: multi-factor (recency, label importance, keyword match, embedding similarity)

### Retrieval Pipeline (`retrieval_provider.rs`)

Coordinates the actual retrieval:
- `RetrievalRequest` — what to search for
- `RetrievedMemoryCandidate` — result with score and snippet
- Integrates with `MemoryStore::search()` (which blends `rg` + LanceDB)

### Compaction (`compaction_view.rs`)

Handles summarizing/converting old context to stay within token limits.
Triggered by `PromptMode::Compact`.

## PromptMode Variants

- `Full` — complete context assembly
- `Compact` — trimmed version for token budget management
- `Planning` — minimal context for planning sub-loops

## Files

| File | Purpose |
|---|---|
| `context/assembler.rs` | Main entry: builds EffectivePrompt with injections |
| `context/memory_selection.rs` | Memory source selection + candidate scoring |
| `context/retrieval_provider.rs` | Retrieval request/result types |
| `context/retrieval_view.rs` | Display rendering for retrieval |
| `context/retrieved_memory_render.rs` | TUI card rendering for memory |
| `context/compaction_view.rs` | Compaction progress display |
| `context/file_search_provider.rs` | File search integration |
| `context/mod.rs` | Module exports |

## Known Gaps

- Provider cache-prefix stability: prompt sections should be stable-ordered
  across turns to maximise hosted-provider prompt-cache hit rate
- Subagent context optimization: subagents get full context, should be trimmed
- No compaction pipeline: code is scattered across `agent/compact/` + `context/`
