# Prompt Runtime Specification

## Problem

RARA originally built its system prompt inline inside the agent and treated compaction as a generic
summary request. That made prompt composition hard to reason about, hard to override, and difficult
to align with the prompt-management patterns used by mature coding agents.

## Scope

- Effective system prompt assembly for normal agent turns.
- Prompt source discovery from workspace and runtime state.
- Prompt override and append behavior from config.
- Dedicated prompt handling for context compaction.
- Shared runtime bootstrap wiring for prompt runtime inputs.

## Non-Goals

- Codex-style state DB driven instruction layering.
- Prompt caching or token-level prompt reuse.
- Full coordinator / worker prompt families.

## Architecture

### 1) Prompt Runtime Inputs

The effective prompt may draw from:

- the default built-in prompt family;
- a configured custom system prompt;
- an optional append prompt;
- workspace instruction files;
- local memory files;
- protocol-registered prompt sources routed through the runtime control plane;
- runtime context;
- plan-mode specific guidance.

### 2) Base Prompt Selection

- If `system_prompt` or `system_prompt_file` is configured, that content replaces the default base
  prompt family.
- Otherwise the default built-in prompt family is used.
- Dynamic runtime sections still apply even when a custom base prompt is configured.

### 3) Effective Prompt Composition

The effective prompt is assembled in this order:

1. base prompt;
2. dynamic instruction sources;
3. memory sources;
4. runtime context;
5. mode-specific addenda such as plan mode;
6. append prompt.

### 3.1) Built-In Engineering Workflow Guidance

The default base prompt includes source-grounded engineering workflow guidance for:

- factual verification before claims about repository, PR, CI, provider, or local-tool behavior;
- external-source and web-search selection, including when to prefer local source, GitHub tooling,
  MCP resources, upstream open-source documentation, or fetched web evidence;
- structured tool use, including edit-tool discipline and unfiltered command-output inspection;
- reviewable implementation workflow, focused validation, and PR hygiene;
- Git conflict resolution when conflict markers are present.

Git conflict guidance is intentionally conservative. It tells the model to inspect the current git
state and conflicted file, preserve complementary changes instead of blindly choosing one side, use
structured edits where practical, scan for remaining conflict markers, and run the narrowest relevant
validation before claiming the conflict is resolved.

Large-write guidance follows the same edit-tool boundary:

- use diff-shaped edit tools or `apply_patch` for modifications to existing files;
- reserve `write_file` for new files or intentional complete rewrites after reading an existing file;
- treat failed, truncated, or apparently non-persistent large writes as tool-result failures to
  diagnose, not as a reason to fall back to shell heredocs, redirection, or PTY writes;
- preserve the Codex distinction that heredoc can be a transport for `apply_patch`, while Claude's
  Bash/PowerShell guidance routes ordinary file writes through Write rather than `cat <<EOF`,
  `echo >`, `Set-Content`, or `Out-File`.

External-source guidance is evidence routing, not a requirement to browse for every question. For
repository, branch, PR, CI, local-tool, and local-configuration claims, the model should prefer the
current codebase, git, GitHub tools, or `gh`. For current external facts and open-source software
questions where local source or docs are unavailable, stale, or insufficient, web search is allowed
only when a web-search or web-fetch tool is actually available in the current tool list. The model
should prefer upstream repositories, official docs, release notes, issue trackers, standards, MCP
resources, or project-provided references over secondary summaries, and should fetch/open source
content before treating search results as evidence when a page-open/fetch tool is available. If web
tools are unavailable or fail, the model should report that limitation instead of pretending to
browse.

### 4) Compact Prompt

- Context compaction must not use the normal system prompt.
- Compaction uses a dedicated compact instruction.
- `compact_prompt` or `compact_prompt_file` overrides the built-in compact instruction.

## Contracts

### 1) Prompt Observability

- The TUI status view must be able to report:
  - whether the base prompt is default or custom;
  - which prompt sections are active;
  - which prompt sources participated in assembly.
- The prompt inspection surface must preserve assembly order and explain for each injected source:
  - what kind of source it was;
  - the display path or source label;
  - why it was included.
- The same source-aware inspection surface should also describe any active compacted-history inputs
  that still contribute to the current turn, including:
  - compaction boundary metadata;
  - structured compacted summaries;
  - recent-file carry-over;
  - recent-file excerpt carry-over.
- The same inspection surface should expose memory/retrieval readiness separately from active
  prompt injection so the runtime can distinguish:
  - sources that are active now;
  - sources that are available for recall;
  - sources that are not currently available.
- The same inspection surface should also show which memory-like items are actually active in the
  current turn, starting with:
  - active workspace memory files that were injected into the effective prompt;
  - compacted thread-memory carry-over such as structured summaries and recent-file carry-over.
  - selected retrieval results reconstructed from retrieval-tool outputs when the current turn has
    already performed explicit recall.
- The same inspection surface should expose the Stage 1 context-assembly result through one shared
  runtime object so `/status`, `/context`, and restore-time runtime snapshots read the same:
  - ordered assembly entries;
  - inclusion and dropped reasons;
  - budget-impact breakdown per layer.
- Session restore must rebuild the same prompt/runtime surface that a direct run would produce for
  persisted session-scoped state such as execution mode, append prompt text, and prompt warnings.

### 2) Workspace Prompt Sources

- Workspace instructions and local memory are treated as explicit prompt sources instead of opaque
  text blobs.
- Prompt source discovery must remain reusable across agent runtime and TUI status reporting.

### 3) Protocol Prompt Sources

- ACP, Wire, and future appserver integrations may register prompt-affecting
  material only through structured prompt source objects.
- Protocol prompt sources must carry provenance, source id, scope, lifetime,
  layer or priority, and budget metadata.
- Protocol adapters must not concatenate raw text directly into the system
  prompt or rename top-level prompt sections.
- `/context`, `/status`, and protocol output subscribers must be able to inspect
  the same prompt source objects.

### 4) Agent Loop Integration

- `Agent::build_system_prompt()` must delegate to the prompt runtime instead of hand-building the
  prompt inline.
- Compaction must pass the dedicated compact instruction down to every backend summarization path.

### 5) Runtime Bootstrap Contract

- Runtime/bootstrap callers must initialize workspace, prompt runtime config, skills, and tools
  through one shared entrypoint instead of wiring those pieces independently in `main.rs` and TUI
  rebuild paths.
- The shared bootstrap entrypoint is `initialize_rara_context(...)` in `src/runtime_context.rs`.
- Bootstrap warnings from prompt/runtime configuration or skill loading must remain visible to the
  caller instead of being silently dropped.
- Workspace-scoped persistence paths used by bootstrap-owned tools should derive from the resolved
  workspace data directory rather than hard-coded literals such as `data/lancedb`.

## Validation Matrix

- `cargo check`
- focused prompt runtime tests for source discovery and prompt precedence
- focused agent tests for compaction instruction wiring

## Open Risks

- The current runtime is closer to Claude-style prompt management than Codex-style instruction and
  state layering.
- Prompt observability now exists in both `/status` and `/context`, including active selected
  workspace/thread memory items, but deeper memory inspection still needs to cover real recalled
  vector/thread selection instead of only prompt-injected or compacted carry-over.
- Protocol-registered prompt sources need strict provenance and lifetime rules
  before ACP/Wire can safely control prompt material.

## Source Journals

- [2026-04-17-prompt-runtime](../journal/2026-04-17-prompt-runtime.md)
- [2026-04-24-context-observability-and-restore](../journal/2026-04-24-context-observability-and-restore.md)
- [2026-04-25-context-assembly-stage1](../journal/2026-04-25-context-assembly-stage1.md)
- [2026-05-02-git-conflict-prompt-guidance](../journal/2026-05-02-git-conflict-prompt-guidance.md)
- [Runtime Control Plane](runtime-control-plane.md)
