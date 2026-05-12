# Skill Tool

## Problem

RARA has a basic `skill` tool and skill discovery implementation, but the contract is still much
smaller than the skill systems in Codex and Claude Code.

Current RARA behavior can list and invoke markdown skills. The next step is to make the skill surface
source-aware, budget-aware, and explicit enough that `verify`, `/review`, repository skills, and
future plugin-style extensions can rely on it without unstable prompt text or hidden precedence.

## Scope

- Skill discovery roots and precedence.
- Skill metadata and parse errors.
- Model-facing available-skill summaries.
- `SkillTool` invocation semantics.
- Skill invocation context injection.
- Status/context observability.
- Compatibility with Codex-style and Claude-style skill files.

## Non-Goals

- A remote marketplace.
- Full Claude plugin compatibility.
- Executing skill-provided shell hooks automatically.
- Letting skills override built-in tools, sandbox policy, or approval rules.
- Treating slash commands and skills as the same public command namespace in the first cut.

## Architecture

### 1) Discovery Roots

RARA should keep its current roots and make them explicit in metadata:

1. home/global roots:
   - `~/.rara/skills`
   - `~/.agents/skills`
2. repo-local roots:
   - `.agents/skills` from repository root toward the current working directory
3. current working directory root:
   - `<cwd>/.rara/skills`
4. system/bundled roots, when added:
   - RARA-owned `.system` skills or packaged skill assets

Each loaded skill should carry a scope:

- `home`
- `repo`
- `cwd`
- `system`
- later: `plugin`

### 2) File Format

RARA should support both existing lightweight markdown skills and frontmatter skills:

```markdown
---
name: verify
description: Verify runtime behavior through the real surface.
---

# Verify

...
```

For compatibility:

- `SKILL.md` is the preferred format.
- Legacy `name.md` remains supported.
- Frontmatter `name` is the invocation identifier and should override the path-derived name.
- Frontmatter `title` or `display_name` is a human-facing display label when present.
- Frontmatter `description` is the model-visible capability summary and should override
  Markdown-derived fallback text.
- For legacy markdown without frontmatter, the first Markdown heading is a display-title and
  description fallback. The loader should extract this into structured metadata; the model should
  not be asked to infer it from raw Markdown.
- Malformed frontmatter should produce a skill load error visible in `/context` or `/status`.

Optional metadata to preserve for future use:

- `short_description`
- `allowed_tools` or `dependencies.tools`
- `allow_implicit_invocation`
- `interface`
- `source`

Unsupported metadata should be retained for display/debugging when practical, but it should not
change runtime behavior until RARA owns that behavior.

### 3) Precedence

Discovery must be deterministic.

Within one root:

- sort skill files by normalized skill name and path;
- when both `name.md` and `name/SKILL.md` exist, `name/SKILL.md` wins.

Across roots:

- higher-precedence roots override lower-precedence roots for the same skill name;
- overridden skills should be retained as metadata so `/context` and `/status` can explain the
  override.

RARA's current effective order lets later roots win. The desired precedence is:

1. system/bundled defaults
2. home/global skills
3. repo-local `.agents/skills` from repository root toward cwd
4. cwd-local `.rara/skills`

This keeps RARA's source-aware roots explicit while preserving the existing repo/cwd override
behavior. RARA should not import `~/.codex/skills` as a default compatibility root.

### 4) Available Skills Prompt

The model-facing skills section should stay compact and stable:

- list skill name;
- list description or short description;
- list display path or root alias;
- include root aliases when absolute paths are too noisy;
- omit or truncate descriptions under budget pressure;
- report omissions as warnings rather than silently hiding budget loss.

The model should be told:

- use a skill when the user names it or the request clearly matches it;
- call `skill` before doing task-specific work when a matching skill exists;
- only invoke skills listed in the current available-skills section or explicitly typed by the user;
- do not invent skill names from memory or training data;
- do not claim that a skill applies unless it has been invoked or already injected in the current
  turn;
- if a skill is already injected in the current turn, follow it instead of invoking it again.

This is closest to Claude Code's `SkillTool` invocation rule and Codex's explicit
`<skills_instructions>` developer fragment.

### 5) SkillTool Invocation

The `skill` tool should support:

- `list`: return available skill metadata, including scope, source path, enabled state, and load
  errors;
- `invoke`: return a specific skill's full instructions;
- optional `args`: pass user-provided arguments to skills that behave like slash-command skills;
- later `reload`: force a discovery reload after file changes.

The invocation result should be structured:

```json
{
  "skill": "verify",
  "path": ".agents/skills/verify/SKILL.md",
  "scope": "repo",
  "instructions": "...",
  "args": "...",
  "warnings": []
}
```

The runtime should inject invoked skill content as a bounded, tagged context item rather than plain
assistant prose. The Codex-like shape is:

```xml
<skill>
<name>verify</name>
<path>.agents/skills/verify/SKILL.md</path>
...
</skill>
```

### 6) Observability

`/context` and `/status` should show:

- skill roots in precedence order;
- active loaded skills;
- overridden skills;
- disabled skills, once config support exists;
- parse/load errors;
- invoked skills in the current or compacted turn;
- budget omissions or truncation.

Skill observability should come from the same source objects used for prompt assembly and `SkillTool`
responses, not from a separate best-effort scan.

## Contracts

### Invocation Contract

- A skill is local instruction content, not executable code.
- `SkillTool` does not bypass sandbox or approval policy.
- `SkillTool` must reject missing skill names with a structured error.
- Skill invocation must be idempotent within a turn: if the skill has already been injected, the
  agent should follow it rather than reinvoking it.
- Matching a visible skill is a pre-task invocation requirement: the model should load the skill
  before doing task-specific analysis, planning, or implementation that the skill governs.
- Skills may request additional tools, but those requests are advisory until RARA implements
  dependency-aware tool gating.

### Compatibility Contract

- RARA-native `.agents/skills` should be discoverable across home and repository roots.
- Claude-style `.claude/skills` may be considered in a later compatibility pass, but native RARA
  repo skills remain `.agents/skills`.
- Claude-style `verifier-*` skills should work as ordinary RARA skills when copied into
  `.agents/skills`.
- Built-in slash commands remain local commands. A skill named `review` must not silently override
  `/review`.

### Budget Contract

- Available-skill summaries must have a bounded context budget.
- Under budget pressure, truncate descriptions before omitting whole skills.
- Report omitted or truncated skill metadata through prompt warnings or `/context`.
- Full skill bodies should only be injected when the skill is invoked or explicitly selected.

## Validation Matrix

- Skill loader tests for frontmatter parsing, legacy markdown fallback, and malformed frontmatter.
- Precedence tests for home, repo, nested repo, cwd, and future system roots.
- Tests proving overridden skills are retained in metadata.
- Prompt tests proving available skills remain sorted and budget-bounded.
- SkillTool tests for `list`, `invoke`, missing skill, and optional `args`.
- `/context` or `/status` tests once skill observability is surfaced.

## Open Risks

- RARA currently stores only the winning skill per name, so override observability requires a metadata
  model change.
- Skill invocation is currently a tool result, not a first-class context fragment with compaction
  recovery.
- Supporting Claude `.claude/skills` directly could create precedence confusion with `.agents/skills`;
  it should be explicit and source-labeled if added.
- Dependency metadata can be displayed before it is enforced, but the UI must not imply missing
  enforcement exists.

## Source Journals

- [2026-05-01-skilltool-and-verify-specs](../journal/2026-05-01-skilltool-and-verify-specs.md)
