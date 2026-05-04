# Web Search Prompt Guidance

## Context

RARA already had base-prompt guidance for factual verification, source-grounded codebase search,
GitHub/PR hygiene, and command-output validation. The missing piece was explicit evidence routing for
external sources and web search.

Codex and Claude Code split this responsibility across tool capability, tool descriptions, and
domain-specific guidance:

- Codex exposes web search through configurable modes and prefers MCP resources when they can answer
  the question.
- Claude Code attaches detailed behavior to its WebSearch tool prompt, including use for recent
  information and required source reporting.

## Change

The default RARA base prompt now includes `external_sources_and_web_search`.

The section tells the model to:

- prefer local source, git, GitHub tools, or `gh` for repository, branch, PR, CI, local-tool, and
  local-configuration claims;
- use web search only when web-search or web-fetch tools are actually available in the current tool
  list;
- use web search when current external facts are needed or when the user explicitly asks to search,
  subject to tool availability;
- allow web search for open-source software questions when web tools are available and local source or
  docs are unavailable, stale, or insufficient;
- prefer upstream repositories, official documentation, release notes, issue trackers, standards,
  MCP resources, and project-provided references before secondary summaries;
- treat search results as an index rather than proof;
- cite sources when web evidence materially supports the answer;
- distinguish verified facts from inferences and assumptions.

The remaining Claude/Codex-aligned behavior is tracked in `docs/features/web-tools.md` and
`docs/todo.md` instead of being folded into this prompt-only change. Those follow-ups include
capability-aware prompt injection and tool registration, source reporting enforcement, current-date
query hints, domain filters, bounded search-use budgets, structured runtime events, auxiliary-model
search execution, and provider-native web search mode support.

## Validation

Focused prompt tests assert that the new section key and core evidence-routing rules are present in
the assembled default system prompt.
