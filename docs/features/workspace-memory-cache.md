# Workspace Memory Cache

## Problem

Workspace memory and prompt instruction files are stable context sources, but
their discovery and cache behavior need a clear contract. Without one, prompt
assembly, `/status`, `/context`, and future ACP/Wire clients can disagree about
which files are active after cwd changes, branch changes, nested workspace
changes, or environment updates.

This is especially sensitive for provider prefix caches: workspace memory sits
near the stable prompt prefix, so nondeterministic invalidation can turn small
workspace changes into avoidable cache misses.

## Scope

This spec defines cache invalidation rules for workspace memory-like prompt
sources.

In scope:

- user and project instruction files;
- local memory files discovered as prompt sources;
- environment-derived prompt-source metadata;
- cwd, repository, branch, and nested workspace changes;
- shared reporting for prompt runtime, `/status`, `/context`, and future
  protocol subscribers.

Out of scope:

- retrieval ranking for volatile memory candidates;
- LanceDB memory record storage;
- session-shard search;
- compaction summaries;
- provider cache-edit APIs.

## Architecture

Workspace memory cache belongs to prompt-source discovery. It is upstream of
retrieval orchestration and `MemorySelection`.

The layering is:

1. resolve workspace identity;
2. discover prompt sources and local memory files in deterministic order;
3. cache discovery results with an invalidation key;
4. expose the resulting source manifest to prompt assembly, `/status`,
   `/context`, and protocol output;
5. let retrieval orchestration consume the manifest only as an input, not as a
   separate discovery path.

Stable prompt prefix order must remain:

1. system prompt;
2. tool schemas;
3. stable skills and project memory;
4. compacted history and carry-over;
5. retrieval and volatile recent context;
6. latest user input.

Workspace memory cache invalidation must not reorder earlier prompt layers.

## Contracts

### Cache Key

The workspace memory cache key should include:

- resolved workspace root;
- current cwd when it changes the instruction-file search path;
- repository identity when available;
- git branch or detached-head commit identity when available;
- prompt-source file paths;
- file metadata needed to detect content changes;
- relevant environment-derived context fields that affect prompt sources.

The cache key should not include volatile turn data such as latest user input,
tool results, retrieval query text, pending approvals, or model output.

### Invalidation Rules

Invalidate cached workspace prompt-source discovery when any of these change:

- cwd crosses into a different workspace root;
- cwd moves into or out of a nested instruction scope;
- git repository root changes;
- git branch or detached-head identity changes;
- a discovered prompt-source file is created, removed, renamed, or modified;
- a parent directory that can contain prompt-source files changes;
- the configured RARA home or workspace data directory changes;
- environment metadata used by prompt-source rendering changes.

Do not invalidate workspace memory cache for:

- latest user message changes;
- tool output changes;
- retrieved-memory ranking changes;
- compaction events;
- model usage counters;
- transient TUI focus or terminal width changes.

Those belong to volatile context, compaction, status, or display views.

### Ordering

Cache rebuilds must produce deterministic ordering:

1. user-level sources;
2. repository-root sources;
3. nested workspace sources from root toward cwd;
4. protocol-registered stable sources ordered by runtime source policy;
5. local memory sources in stable path order.

Hash maps must not determine externally visible ordering.

### Observability

The runtime should expose a workspace source manifest with:

- source id;
- source kind;
- display path or label;
- scope;
- provenance;
- cache key fragment or version;
- inclusion reason;
- dropped or unavailable reason where applicable.

`/status`, `/context`, and ACP/Wire subscribers should read the same manifest.
They should not re-run independent discovery logic.

### Relationship To MemorySelection

Workspace memory cache answers "which stable workspace sources exist and are
active." `MemorySelection` answers "which memory-like items are selected,
available, or dropped for this turn."

Stable workspace memory can appear in `MemorySelection` as a fixed selected
item for explainability, but it must not compete with volatile retrieval
candidates for discretionary retrieval budget.

## Validation Matrix

- cwd changes that do not cross instruction scope do not reorder stable sources.
- cwd changes into a nested instruction scope add only the nested suffix.
- git branch changes invalidate prompt-source discovery.
- prompt-source file create/remove/modify invalidates discovery.
- latest user input and tool output do not invalidate workspace memory cache.
- `/status` and `/context` report the same prompt-source manifest.
- tests assert deterministic ordering independent of hash-map iteration.

## Open Risks

- File watching may be platform-specific. The first implementation can use
  explicit rebuild triggers and metadata checks before adding watchers.
- Git branch detection can be expensive if done on every render path. Cache
  invalidation should happen during runtime refresh, not TUI drawing.
- Protocol-registered stable sources need clear lifetimes so they do not create
  prefix churn across turns.

## Source Journals

- [Prompt Runtime](prompt-runtime.md)
- [Context Architecture](context-architecture.md)
- [Memory Selection](memory-selection.md)
- [Retrieval Orchestration](retrieval-orchestration.md)
