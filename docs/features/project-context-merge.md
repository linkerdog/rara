# Project Context Merge

## Problem

RARA currently injects project-level instructions and session memory as two
separate dynamic prompt sections (`instructions` and `memory`).  Claude Code
takes the opposite approach: all file-based context (CLAUDE.md, auto-memory,
rules) is merged into a single `claudeMd` string with labeled sub-sections,
injected into the system prompt as one `## Memory section` block.

This split model has downsides:

- The model sees instructions and memory as disconnected regions, leading to
  weaker cross-referencing between project rules and remembered facts.
- Adding a new injection path (e.g. charter sync) requires deciding which
  section it belongs to, introducing fragmentation.
- Two sections with small dynamic content each creates section bloat.

## Scope

- Merge the current `instructions` and `memory` dynamic sections into a single
  `project_context` section.
- Sub-sections within `project_context` mirror Claude Code's labeling:
  `### Project Instructions` and `### Session Memory`.
- Accept a one-time provider-cache invalidation.
- Do not change content sources, discovery, or `WorkspaceCache` behavior.

## Non-Goals

- Changing how `ProjectInstruction`, `UserInstruction`, or `LocalMemory`
  sources are discovered or cached.
- Adding the memdir-style semantic retrieval path (that is a separate feature).
- Changing `PromptSourceKind` variants or their semantics.
- Changing the `DYNAMIC_BOUNDARY` or static prefix sections.

## Architecture

### Section before

```
static prefix
...
__DYNAMIC_BOUNDARY__
## instructions
  ## <UserInstruction.label>
  ## <ProjectInstruction.label>
## memory
  ## <LocalMemory.label>
## protocol_prompt_sources
## skills
## language_best_practices
## runtime_context
## execute_mode / plan_mode / review_mode
```

### Section after

```
static prefix
...
__DYNAMIC_BOUNDARY__
## project_context
  ### Project Instructions
  <UserInstruction content>
  <ProjectInstruction content>
  ### Session Memory
  <LocalMemory content>
## protocol_prompt_sources
## skills
## language_best_practices
## runtime_context
## execute_mode / plan_mode / review_mode
```

### Implementation

In `crates/instructions/src/prompt.rs`, the function that builds dynamic
sections currently does:

```rust
let instruction_block = /* concat UserInstruction + ProjectInstruction sources */;
let memory_block = /* find LocalMemory source */;

vec![
    PromptSection::optional("instructions", instruction_block),
    PromptSection::optional("memory", memory_block),
    PromptSection::optional("protocol_prompt_sources", protocol_prompt_sources_block),
    PromptSection::optional("skills", skills_block),
    PromptSection::optional("language_best_practices", language_prompt),
    PromptSection::new("runtime_context", render_environment_context(&cwd, &branch)),
    PromptSection::optional("execute_mode", ...),
    PromptSection::optional("plan_mode", ...),
    PromptSection::optional("review_mode", ...),
]
```

After the change, the two blocks are merged into one:

```rust
let project_context_block = build_project_context_block(sources);

vec![
    PromptSection::optional("project_context", project_context_block),
    PromptSection::optional("protocol_prompt_sources", protocol_prompt_sources_block),
    PromptSection::optional("skills", skills_block),
    PromptSection::optional("language_best_practices", language_prompt),
    PromptSection::new("runtime_context", render_environment_context(&cwd, &branch)),
    PromptSection::optional("execute_mode", ...),
    PromptSection::optional("plan_mode", ...),
    PromptSection::optional("review_mode", ...),
]
```

Where `build_project_context_block`:

1. Collects `UserInstruction` and `ProjectInstruction` sources — if any exist,
   renders them under `### Project Instructions`.
2. Finds the `LocalMemory` source — if it exists, renders it under
   `### Session Memory`.
3. Wraps both sub-sections under `## Project Context`.
4. Returns `None` when both sub-sections are empty (preserving the existing
   behavior where absent sources produce no section).

### Prefix stability

The change reduces the dynamic section count from 9 to 8.  Section keys change
from `[instructions, memory, ...]` to `[project_context, ...]`.  This is a
**one-time cache invalidation** for every provider.  After the merge, the
`project_context` section becomes the new stable prefix boundary, and future
changes within it (new source content) do not invalidate again (the section
key stays the same).

## Contracts

| Contract | Detail |
|----------|--------|
| **Merged section** | `project_context` is the only section for file-based context. |
| **Sub-section labeling** | `### Project Instructions` for instruction sources, `### Session Memory` for memory sources. |
| **Empty handling** | If no instruction sources AND no memory sources, `project_context` is absent (no empty section rendered). |
| **Source kinds unchanged** | `UserInstruction`, `ProjectInstruction`, `LocalMemory` remain as is; only the renderer changes. |
| **Section count** | Dynamic sections reduce by 1 (two sections → one). |
| **DYNAMIC_BOUNDARY** | Unchanged. `project_context` is below the boundary, as instructions and memory were. |

## Validation Matrix

| Check | Method | Expected |
|-------|--------|----------|
| Project instructions appear under `### Project Instructions` | Unit test with instruction sources | Content wrapped with sub-section header |
| Session memory appears under `### Session Memory` | Unit test with LocalMemory source | Content wrapped with sub-section header |
| No LocalMemory source | Unit test | `### Session Memory` sub-section absent |
| No instruction sources | Unit test | `### Project Instructions` sub-section absent |
| Both absent | Unit test | `project_context` section absent |
| Section count | Assert on vec len | 8 instead of 9 |
| `cargo build` | `cargo build` | No new warnings |
| `cargo test` | `cargo test` | All existing tests pass, snapshot tests may need update |
| Prefix cache key change | Manual: inspect `section_keys` | `project_context` replaces `instructions` + `memory` |

## Operational Notes

- This is a one-time break in provider prefix caches.  The new section key
  `project_context` becomes the stable boundary going forward.
- Snapshot tests that assert on the full prompt output will need regeneration.
- Existing journal entries referencing `instructions` or `memory` section keys
  are historical records and do not need updating.

## Source Journals

- _(this spec, written before implementation)_
