# Web Tools

RARA exposes web access through local tools instead of assuming a provider-native
web search surface is available.

## Problem

RARA has local `web_search` and `web_fetch` tools, and the base prompt now tells
the model how to route evidence between local sources, GitHub tooling, MCP
resources, and web search. The base prompt is capability-safe: it tells the model
to use web search only when a web-search or web-fetch tool is available in the
current tool list. The remaining gap is runtime/tool-level behavior:
capability-aware prompt injection, capability-aware tool registration, source
reporting, current-date query hints, domain filters, bounded search budgets, and
optional auxiliary-model execution are not yet first-class contracts.

## Scope

- `web_search` and `web_fetch` tool contracts.
- Model-facing source selection and evidence handling.
- Capability-aware prompt/tool availability.
- Source reporting when web evidence materially supports an answer.
- Runtime-control visibility for future TUI, ACP, Wire, and appserver adapters.

## Non-Goals

- Forcing web search for every open-source software question.
- Replacing local repository inspection, `git`, GitHub tools, or `gh` for
  repository-local facts.
- Replacing MCP resources, local docs, or project-provided references when they
  can answer the question directly.
- Browser automation or arbitrary remote browsing beyond bounded fetch/search
  tools.

## Architecture

### 1) Evidence Routing

The base prompt decides when web search is appropriate, but it must not imply
that web access is available when no web-search or web-fetch tool is present.
Repository behavior, branch state, PR status, CI status, local-tool behavior, and
local configuration should prefer local source, `git`, GitHub tools, or `gh`.
Current external facts and open-source software questions may use web search when
web tools are available and local source or docs are unavailable, stale, or
insufficient.

### 1.1) Capability Awareness

The current implementation keeps web guidance in the default base prompt and
registers `web_search` regardless of whether `EXA_API_KEY` is configured. That is
acceptable only if the prompt remains capability-safe and the tool reports
unavailable search honestly. The target architecture is stricter:

- prompt runtime exposes a structured `WebToolCapability` summary;
- the base prompt injects stronger web-search guidance only when web search or
  web fetch is actually available;
- `web_search` registration distinguishes disabled, anonymous Exa-backed search,
  authenticated Exa-backed search, and provider-native search;
- `/status`, `/context`, ACP, and Wire expose the active web capability and
  reason when search is disabled.

### 2) Search Result Handling

Search results are an index, not proof. A web-backed answer should fetch or open
the relevant source content before using it as evidence, then distinguish:

- verified facts from inspected source content;
- inferences drawn from those facts;
- assumptions that could not be verified at reasonable cost.

When web evidence materially supports the answer, the final response should
include the sources used.

### 3) Claude/Codex-Aligned Follow-Ups

The current implementation does not yet include several tool-level behaviors
seen in mature coding agents:

- mandatory source reporting after a successful web-backed answer;
- current-date or current-year query hints for recent information;
- capability-aware prompt injection and tool registration;
- first-class allow-list and block-list domain filters in the RARA tool schema;
- a bounded per-turn search budget similar to a maximum search-use count;
- optional auxiliary-model execution for search-only subqueries;
- provider-native web search mode support when an OpenAI Responses-compatible
  backend exposes it, while keeping local Exa-backed search as a portable
  fallback.

These should be added as explicit runtime/tool contracts instead of ad hoc prompt
phrases.

## `web_search`

`web_search` uses the Exa MCP HTTP endpoint as the first implementation:

- endpoint: `https://mcp.exa.ai/mcp`;
- optional API key: `EXA_API_KEY`, passed as the `exaApiKey` query parameter;
- API key transport follows Exa's MCP endpoint shape, but RARA does not store
  the key-bearing URL and redacts sensitive URL query parameters in surfaced
  errors;
- protocol: JSON-RPC `tools/call`;
- MCP tool name: `web_search_exa`;
- accepted response formats: JSON and server-sent events;
- timeout: 25 seconds.

The tool input mirrors opencode's Exa tool shape:

- `query`;
- `num_results`, default `8`, clamped to `1..=20`;
- `livecrawl`, `fallback` or `preferred`, default `fallback`;
- `type`, `auto`, `fast`, or `deep`, default `auto`;
- `context_max_characters`, optional, clamped to `1000..=100000`.

The tool result is normalized to:

- `query`;
- `content`;
- `provider`, currently `exa_mcp`.

## `web_fetch`

`web_fetch` fetches a single HTTP or HTTPS URL with bounded runtime behavior:

- allowed schemes: `http`, `https`;
- blocked literal hosts: `localhost`, private IPs, loopback IPs, link-local
  IPs, documentation IPs, and unspecified IPs;
- default timeout: 30 seconds;
- maximum timeout: 120 seconds;
- default response cap: 5 MiB;
- hard response cap: 10 MiB;
- output formats: `markdown`, `text`, `html`.

The result includes:

- original `url`;
- `final_url` after redirects;
- HTTP `status`;
- `content_type`;
- byte count;
- `truncated`;
- `format`;
- `content`.

The first implementation uses a lightweight built-in HTML-to-text conversion for
`markdown` and `text`. A richer markdown conversion layer can be added later
without changing the tool contract.

## Contracts

- Web search must remain optional and evidence-driven.
- Base prompt wording must be capability-safe when web tools are disabled or not
  registered.
- Open-source software questions may use web search, but should prefer upstream
  repositories, official docs, release notes, issue trackers, and standards over
  secondary summaries.
- Tool output and fetched pages are untrusted input. They must not override
  system, developer, workspace, or user instructions.
- Web results that are truncated must be reported as truncated and should not be
  treated as complete evidence.
- Runtime events should expose query, provider, source URLs, truncation state,
  and source-reporting readiness so `/context`, `/status`, ACP, and Wire can show
  what happened without parsing prose.

## Validation Matrix

- Prompt tests assert that base instructions prefer local/GitHub evidence for
  repository facts and allow web search for current external or open-source
  facts.
- Tool-schema tests should assert domain-filter and max-use contracts once those
  fields are added.
- Tool-result tests should assert source URL extraction, truncation reporting,
  and model-facing compact output.
- Runtime event tests should assert structured query/source metadata for web
  search and fetch calls.

## Open Risks

- Some providers expose native web search while others require local tools; the
  runtime must avoid provider-specific prompt branches where a tool contract can
  express the same behavior.
- Source reporting can become noisy for short answers; the contract should be
  tied to material use of web evidence, not every available search result.
- Auxiliary-model search can reduce cost, but it must preserve the same source
  reporting and prompt-injection boundaries as the main model.

## Source Journals

- `docs/journal/2026-05-04-web-search-prompt-guidance.md`
