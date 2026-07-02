# Repository Extension Surface

## Goal

RARA should recognize repository-scoped customizations from multiple agent
ecosystems without immediately coupling discovery to execution.

The initial target is compatibility with:

- repo context files that describe local project rules, conventions, and active
  workspace facts;
- native RARA skills under `.agents/skills/`;
- Claude-compatible agent definitions under `.rara/agents/`;
- Claude-style lifecycle hooks under `.claude/hooks/`.

This document defines discovery, precedence, compatibility boundaries, and a
staged rollout order.

It does **not** require full runtime execution support in the first cut.

## Related Claude Code Capabilities

This extension surface is intended to align with the Claude Code capabilities
that are already meaningful at repository scope:

- repo-local agent definitions under `.claude/agents/`;
- repo-local lifecycle hooks under `.claude/hooks/`;
- a sub-agent model where imported agent roles are more structured than plain
  prompt snippets;
- hookable runtime phases around prompt submission, tool execution, and session
  lifecycle.

For RARA, the compatibility target is the **shape of the extension surface**,
not a byte-for-byte clone of Claude Code runtime semantics.

That means:

- RARA may normalize Claude-format agent files into RARA-owned imported-agent
  objects;
- RARA may normalize Claude hook declarations into RARA-owned hook-definition
  objects;
- actual execution still has to respect RARA thread, context, sandbox, and tool
  boundaries.

The Claude-related runtime phases that map most cleanly onto current RARA
surfaces are:

- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`
- `Stop`

Later follow-up phases may include:

- `SubagentStop`
- `PreCompact`

## Related Codex Capabilities

Codex has a different extension shape, but the same compatibility pressure:
repository guidance, local skills, runtime memory, and tool/result context must
compose without making the model-facing prompt unstable across releases.

RARA should therefore treat Codex-style inputs as additional normalized context
sources instead of as a reason to rename or reshuffle the primary prompt
sections.

The Codex-related behaviors to preserve are:

- repository instructions and repo-local skills participate in the same source
  discovery pipeline as other workspace prompt sources;
- durable memory and recalled thread/workspace facts enter through
  `MemorySelection`;
- tool results and runtime state stay structured runtime inputs instead of
  being converted into ad hoc user-text prefixes;
- visible `/context` and `/status` surfaces explain the same source objects that
  were used for model assembly.

## Prefix Stability Contract

Repository context, skills, hooks, imported agents, and memory are expected
long-term inputs for RARA, but their addition must not churn the stable
model-facing prompt/context prefix layout.

The main prefix names and top-level assembly order should stay stable unless a
breaking context-format migration is explicitly planned.

New capabilities should normally enter through one of these owned extension
points:

- a new source object under existing workspace prompt-source discovery;
- a normalized extension object surfaced by this repository extension contract;
- a memory or retrieval candidate produced for `MemorySelection`;
- a lifecycle event handled behind the hook runtime boundary;
- a child-thread or imported-agent profile executed through the thread domain;
- a protocol-registered source routed through the runtime control plane.

They should not:

- inject raw repo files directly into the system prompt with new top-level
  prefixes;
- rename existing stable prompt sections just to match an external ecosystem;
- serialize hook, skill, or agent metadata as ordinary user text;
- add synthetic context prefixes for transient runtime artifacts such as orphan
  tool results;
- let ACP, Wire, or another protocol adapter bypass the same source,
  `MemorySelection`, hook, skill, and permission boundaries used by local
  runtime paths.

If a new source needs a label for explainability, that label should be attached
to the structured source object and rendered by `/context` or `/status`, not
invented as an unowned prompt prefix.

## Non-Goals

The first rollout should not:

- execute arbitrary hook files on discovery;
- treat imported Claude-compatible agents as first-class RARA child threads
  immediately;
- merge all extension types into one generic opaque plugin loader;
- introduce a marketplace or remote-install protocol.

## Core Principle

RARA should separate:

1. **discovery**
   - what repo-local extensions exist;
2. **normalization**
   - how discovered files are mapped into RARA-owned objects;
3. **execution**
   - when and how those normalized objects affect runtime behavior.

Protocol adapters add a fourth entrypoint shape but not a fourth semantic
model. ACP, Wire, local TUI commands, and future appserver integrations should
all adapt into RARA-owned source, hook, skill, memory, thread, and control-event
objects instead of defining parallel extension semantics.

The first milestone should complete discovery and normalization before adding
runtime execution.

## Extension Types

### Native Skills

RARA-native reusable skills live under:

- `.agents/skills/<skill-name>/SKILL.md`

They remain the authoritative skill format for direct runtime use.

### Imported Agent Definitions

RARA-native repo-local agents live under:

- `.rara/agents/*.md`

These files use the same frontmatter-plus-body format as Claude Code's
`.claude/agents/*.md` files. RARA may still import `.claude/agents/*.md` as a
legacy compatibility path, but `.rara/agents` is the canonical RARA location and
has higher precedence for same-name definitions.

RARA should treat these as **importable agent profiles**, not as raw prompt
files.

Each discovered agent definition should normalize into a structured object such
as:

- `ImportedAgentProfile`
  - `id`
  - `label`
  - `source_path`
  - `source_kind = "rara_agent"`
  - `prompt_body`
  - `tools_policy`
  - `description`
  - `parse_status`

The status projection and runtime execution path should share the same
Claude-compatible parser so `ImportedAgentProfile` cannot drift from
`AgentDefinition`.

### Imported Hook Definitions

Claude-style lifecycle hooks live under:

- `.claude/hooks/`

RARA should treat them as **declared hook candidates**, not as executable logic
until the hook runtime contract exists.

Each discovered hook definition should normalize into a structured object such
as:

- `ImportedHookDefinition`
  - `id`
  - `source_path`
  - `declared_event`
  - `handler_kind`
  - `handler_target`
  - `parse_status`

If a hook file cannot be fully parsed, RARA should still surface it as a known
repo extension with a parse/error status instead of silently ignoring it.

## Discovery Roots

RARA should distinguish:

1. user/home-level extension roots;
2. workspace-level extension roots;
3. nested workspace roots when supported by current prompt/workspace discovery.

The initial repository-scoped roots are:

- `<workspace>/.agents/skills/`
- `<workspace>/.rara/agents/`
- `<workspace>/.claude/agents/`
- `<workspace>/.claude/hooks/`

## Precedence Rules

Precedence should be explicit and source-aware.

### Skills

For skill name conflicts:

RARA loads lower-precedence roots first and lets later roots override earlier
skills with the same name. The current order is:

1. home/global skills under `~/.rara/skills` and `~/.agents/skills`
2. repo-local `.agents/skills` from the repository root toward the current
   working directory
3. current-working-directory `.rara/skills`

Within a single root, skill discovery is deterministic. If both `name.md` and
`name/SKILL.md` exist, the directory skill is loaded last and wins.

RARA should surface overridden skills in status/debug output instead of hiding
them.

The detailed `SkillTool` and skill metadata contract is defined in
[Skill Tool](skill-tool.md). This extension-surface document owns the broader
repo compatibility boundary; the skill-tool spec owns invocation, prompt
budgeting, source scopes, and skill-body injection.

Claude-style verification should be handled through ordinary skills first:

- `verify` is the general verification workflow skill;
- `verifier-*` skills are project-specific evidence-capture protocols;
- both should live under RARA's native skill roots, especially
  `.agents/skills/`, until direct `.claude/skills` compatibility is explicitly
  added.

See [Verify Skill](verify-skill.md) for the detailed verification contract.

### Imported Agents

Imported RARA agents should remain in their own namespace and should not
silently override native RARA skills or built-in sub-agent kinds.

That means:

- an imported agent named `planner` should not replace RARA's built-in planning
  mode;
- collisions should be surfaced as compatibility warnings or namespace-qualified
  entries.

### Imported Hooks

Imported hooks should never implicitly override built-in safety/runtime rules.

If a hook later becomes executable, built-in runtime invariants still win:

1. safety and policy constraints;
2. persisted thread/runtime continuity rules;
3. hook execution.

## Status / Explainability Requirements

Once discovery lands, `/status` or equivalent debug surfaces should be able to
show:

- discovered native skills;
- discovered imported RARA/Claude-compatible agents;
- discovered imported Claude hooks;
- source path;
- precedence or override status;
- parse status;
- whether runtime execution is currently supported.

This is required before adding hook execution so the extension surface stays
debuggable.

## Runtime Compatibility Plan

### Stage 1: Discovery Only

Deliver:

- discovery of `.agents/skills/`, `.rara/agents/`, `.claude/agents/`,
  `.claude/hooks/`;
- normalized metadata objects;
- source-aware status reporting;
- precedence/override visibility.

Do not yet:

- run hooks;
- spawn imported agents automatically.

### Stage 2: Imported Agent Profiles

Deliver:

- map imported Claude-compatible agents into explicit RARA agent profiles;
- allow opt-in invocation through a RARA-owned delegation/runtime surface;
- keep parent/child thread contracts owned by `ThreadStore`, not by imported
  file formats.

Imported agent files should adapt into RARA's thread/sub-agent model rather than
define a separate execution path.

### Stage 3: Minimal Hook Runtime

Deliver a minimal hook contract with a small event set, for example:

- `SessionStart`
- `UserPromptSubmit`
- `PreToolUse`
- `PostToolUse`
- `Stop`

The first runtime cut should prefer:

- explicit command hooks;
- explicit prompt-injection hooks;
- deterministic failure/reporting rules.

RARA should avoid broad "run any repo script at any time" semantics.

### Stage 4: Richer Hook / Agent Interop

Possible later work:

- imported agent-specific hook policies;
- MCP- or HTTP-backed hooks;
- repo-local approval or formatting workflows;
- richer compatibility with external agent ecosystems.

This stage should only happen after the core thread/runtime lifecycle remains
stable under Stage 2 and Stage 3.

## Thread and Context Constraints

Imported extensions must not bypass RARA-owned runtime boundaries.

That means:

- imported Claude-compatible agents still run through RARA sub-agent/thread
  contracts;
- imported hooks still operate through RARA lifecycle events;
- repo context and repo-local skills still enter through source discovery and
  context assembly instead of bypassing prompt ownership;
- context injection still flows through `ContextAssembler` and
  `MemorySelection`;
- thread persistence still flows through `ThreadStore` / `ThreadRecorder`.
- third-party protocol control still flows through the runtime control plane
  before reaching prompt, skill, memory, hook, approval, or output domains.

Compatibility should adapt external formats into RARA-owned objects instead of
letting external directory conventions define runtime semantics directly.

## Open Questions

- How much of Claude agent frontmatter or metadata should be preserved vs
  normalized away?
- Should imported hooks live in a separate "compatibility" status section until
  executable support lands?
- Should nested workspace discovery allow multiple `.claude/` roots, or only
  the active workspace root?
- How should imported agent names appear in `/help` or command surfaces without
  colliding with built-ins?

## Immediate Follow-Up

The next implementation slice should stay small:

1. add source-aware discovery objects for repo-local native skills, Claude
   agents, and Claude hooks;
2. expose them in `/status`;
3. record precedence and parse status;
4. add the initial `verify` skill under `.agents/skills/verify/SKILL.md`;
5. defer hook and imported-agent runtime execution to a later milestone.
