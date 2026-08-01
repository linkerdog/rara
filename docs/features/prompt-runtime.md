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
- an optional final subagent capability policy section;
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
5. mode-specific addenda such as execute mode, plan mode, and review mode;
6. append prompt.
7. subagent capability policy section, when a child session is active.

### 3.1) Built-In Engineering Workflow Guidance

The default base prompt includes source-grounded engineering workflow guidance for:

- software-engineering task interpretation for terse repository instructions;
- terminal Markdown output rules for user-facing assistant text;
- factual verification before claims about repository, PR, CI, provider, or local-tool behavior;
- external-source and web-search selection, including when to prefer local source, GitHub tooling,
  MCP resources, upstream open-source documentation, or fetched web evidence;
- structured tool use, including edit-tool discipline and unfiltered command-output inspection;
- reviewable implementation workflow, focused validation, sandbox-persistent verification, and PR
  hygiene;
- Git conflict resolution when conflict markers are present.

Software-engineering task interpretation is adapted from Claude-style task framing. In a repository,
short user requests such as "rename this", "clean it up", "continue", or "review this" should be
interpreted against the current workspace, git state, PR state, project docs, and runtime
context before being treated as abstract prose. The model should inspect and act on discoverable
repository targets rather than returning a text-only transformation.

Terminal Markdown guidance is adapted from Claude Code's terminal harness behavior and Codex's TUI
markdown-rendering contract. The default prompt should tell the model that user-facing text is
GitHub-flavored Markdown rendered in a terminal, match structure to task complexity, avoid headings
for simple answers, use concise bullets for longer reports, use language-tagged fenced code blocks
for multi-line code or commands, prefer `path:line` code references, avoid large tables unless they
improve comparison, and avoid emojis unless explicitly requested.

Shell capability guidance keeps the runtime portable across constrained environments. When a
dedicated tool is unavailable or unsuitable, the agent checks an optional shell command before
depending on it (for example, `command -v rg`), prefers `rg` only when present, and otherwise uses
an equivalent available or POSIX tool. Missing commands are not an implicit package-installation
request: the default prompt must not infer a package manager or install a dependency unless the
user explicitly requests an environment change.

Git conflict guidance is intentionally conservative. It tells the model to inspect the current git
state and conflicted file, preserve complementary changes instead of blindly choosing one side, use
structured edits where practical, scan for remaining conflict markers, and run the narrowest relevant
validation before claiming the conflict is resolved.

Testing and verification guidance is split between the always-on base prompt, execute-mode addenda,
and tool descriptions. The default prompt should require the agent to reproduce or characterize bug
failures before changing code when practical, prefer focused regression tests, inspect the changed
runtime surface when user-visible or workflow behavior changed, and treat sandbox denials as a
diagnostic or escalation path rather than a reason to abandon validation. A denied validation call
or denied escalation request should be treated as new routing information: do not retry it
verbatim, either narrow the command, use another local evidence path, or explain the exact blocked
capability that still requires approval.

Large-write guidance follows the same edit-tool boundary:

- use diff-shaped edit tools or `apply_patch` for modifications to existing files;
- reserve `write_file` for new files or intentional complete rewrites after reading an existing file;
- use `replace` for one exact unique replacement, `replace_lines` for verified line-range edits,
  and `multi_edit` for several related exact replacements in one file;
- treat failed, truncated, or apparently non-persistent large writes as tool-result failures to
  diagnose, not as a reason to fall back to shell heredocs, redirection, or PTY writes;
- preserve the Codex distinction that heredoc can be a transport for `apply_patch`, while Claude's
  Bash/PowerShell guidance routes ordinary file writes through Write rather than `cat <<EOF`,
  `echo >`, `Set-Content`, or `Out-File`.

Tool-schema edit guidance is part of the runtime contract, not only prose in the
base prompt. `bash` should explicitly route file modifications to direct edit
tools. `replace`, `replace_lines`, `multi_edit`, and `apply_patch` should each
state the safe edit boundary they own so the model sees the instruction at the
call site where it chooses a tool.

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

### 3.2) Guidance Placement Rules

Prompt guidance is intentionally split by responsibility:

- Runtime system prompt: behavior that materially affects task completion, correctness, safety,
  evidence quality, memory staleness, and agent-loop continuation.
- Execute-mode addenda: execution-only workflow rules that depend on mutable task progress, such as
  keeping multi-step work current with `todo_write`, carrying pending verification items, and
  biasing toward the next safe local step.
- Plan/review-mode addenda: read-only contracts, approval flow, and output-shape rules that apply
  only in those modes.
- Tool descriptions and input schemas: call-time constraints such as shell-vs-PTY selection,
  dedicated file/edit tool preference, `cwd` handling, background task controls, stdout/stderr
  handling, and sandbox-escalation discipline for validation commands.
- Workspace instruction files such as `AGENTS.md`: repository-maintenance conventions, Rust API
  style, TUI module boundaries, snapshot expectations, commit rules, and documentation workflow.
- Skills: deeper task-specific workflows that should be loaded only when relevant.

This mirrors the Codex/Claude split without copying product-specific internals into every turn.
New always-on prompt text should be rare, additive, and placed near related sections without
reordering existing prompt sections, because stable prefixes matter for provider prompt-cache reuse.
RARA-specific documentation conventions such as SDD, journals, and TODO hygiene belong in
workspace instructions and documentation, not in the default runtime system prompt.

Prompt locality is a contract, not only a style preference:

- place a rule at the narrowest layer that still guarantees the behavior;
- keep the always-on default prompt focused on durable invariants, not detailed tool syntax,
  repository-maintenance procedures, or repeated examples;
- prefer tool descriptions, input schemas, skills, mode addenda, workspace instructions, or dynamic
  context for narrow procedural guidance that is only useful at call time or for a specific task;
- do not move an execution-only rule into the always-on base prompt just because Claude or Codex has
  a nearby sentence in its own runtime;
- when borrowing from other agents, migrate the behavioral contract, not the exact prompt text or
  provider-specific wrapper structure.

Examples:

- irreversible-action confirmation belongs in always-on runtime safety guidance because it applies
  across tasks and tools;
- `todo_write` usage belongs in execute-mode guidance because it is meaningless in read-only plan or
  review turns;
- `todo_write` replacement semantics and completion discipline belong in the tool description because
  the model needs them when deciding what todo payload to send;
- stale-read recovery and file-write boundaries belong in edit-tool descriptions because the model
  needs those rules at the tool-choice point;
- sandbox-escalation behavior for tests, builds, and checks belongs in the bash tool description as
  well as the always-on validation guidance because the model needs that rule when choosing whether
  to retry, narrow the command, or request approval, and because denied calls should not be retried
  verbatim;
- repository-specific engineering rules remain in `AGENTS.md` or skills instead of bloating the
  default runtime prompt.

### 4) Compact Prompt

- Context compaction must not use the normal system prompt.
- Compaction uses a dedicated compact instruction.
- `compact_prompt` or `compact_prompt_file` overrides the built-in compact instruction.
- The built-in compact instruction is a continuation contract, not a generic recap request. It should
  preserve user intent, constraints, repository findings, inspected or touched files, completed work,
  plan state, pending interactions, unresolved risks, failed approaches, and the next best action.
- The structure is intentionally markdown-only in this phase. The runtime should not require a
  Claude-specific wrapper tag unless a future parser owns that format.

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
- `ProtocolPromptSourceSnapshot` is the registry-to-runtime bridge. It converts
  into a `PromptSourceKind::ProtocolPromptSource` entry before prompt assembly,
  preserving the source id in the label, protocol provenance in the display
  path, and adapter-provided content as the source body.
- Protocol prompt sources are rendered in a dedicated dynamic
  `protocol_prompt_sources` section. This keeps workspace instructions,
  workspace memory, skills, runtime environment, and append prompts in their
  existing relative order while still making protocol input visible in the
  effective prompt and prompt-source context view.
- Runtime bootstrap owns the live `PromptSourceRegistry` and attaches it to the
  agent. At the start of each user query, the agent atomically snapshots
  turn-active registry entries into `PromptRuntimeConfig::protocol_prompt_sources`
  and advances turn-limited lifetimes under the same registry lock. The
  snapshotted sources remain active for every model request inside that query's
  agent loop. This keeps live protocol input on the normal prompt-runtime path
  instead of letting adapters edit final prompt text.
- The registry emits lifecycle events for protocol prompt sources:
  `Registered` when accepted, `Injected` when snapshotted into a user query, and
  `Dropped` when a turn-limited source expires or is removed from the registry.

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
- [2026-05-07-engineering-guidance-placement](../journal/2026-05-07-engineering-guidance-placement.md)
- [2026-05-13-claude-prompt-locality](../journal/2026-05-13-claude-prompt-locality.md)
- [2026-05-14-sandbox-denial-escalation-guidance](../journal/2026-05-14-sandbox-denial-escalation-guidance.md)
- [Runtime Control Plane](runtime-control-plane.md)
