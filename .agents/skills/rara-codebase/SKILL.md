---
name: rara-codebase
description: Use when you need to understand the RARA codebase structure — what each file does, where subsystems live, and how they connect. The index maps every source file to its role.
---

# RARA Codebase Map

One-line index of every source file, grouped by subsystem.

## Memory Subsystem

| File | Lines | Role |
|---|---|---|
| `memory_store.rs` | 15 (facade) | Facade including `memory_types`, `memory_store_impl`, `memory_records`, `memory_store_helpers` |
| `memory_types.rs` | 211 | Enums + structs: `MemoryLabel`, `MemorySource`, `MemoryRecord`, `NewMemoryRecord`, `MemoryRecordPatch`, `MemoryRecordSearchHit` |
| `memory_store_impl.rs` | 268 | `MemoryStore` struct + CRUD: `insert`, `search`, `get`, `update`, `delete`, `set_pinned`, `list_recent` |
| `memory_records.rs` | 185 | `PersistedMemoryRecordFile` + `MemoryRecordFileStore`: file-based persistence with atomic writes |
| `memory_store_helpers.rs` | 372 | Utilities: `clamp`, `truncate_string`, `sort_memory_records`, `default_record_path_for_vdb_uri`, `MemoryMetadata` |
| `memory_store_tests.rs` | 587 | 17 integration tests for insert/read/update/delete/search cycles |
| `memory_distiller.rs` | 315 | LLM-powered memory distillation: `distill_thread_markdown()`, `dedupe_memory_drafts()`, `new_memory_record_from_draft()` |
| `memory_files.rs` | 9 (shim) | Re-export from `rara-memory` crate — see `crates/rara-memory/src/files.rs` |
| `memory_notice.rs` | 12 | `memory_notice()` formatter for in-context memory action messages |
| `crates/rara-memory/src/files.rs` | 415 | File-based memory operations: `write_memory()`, `read_summary_for_context()`, `update_summary()`, `search_memory()`, atomic writes + fs2 locks |
| `crates/rara-memory/src/vectordb.rs` | 619 | LanceDB vector database layer: `VectorDB`, `MemoryMetadata`, schema management |

## Context Subsystem

| File | Lines | Role |
|---|---|---|
| `context/assembler.rs` | 112 | `ContextAssembler`: builds effective prompt with memory + hooks injection |
| `context/memory_selection.rs` | 452 | Workspace memory selection: `available_memory_sources`, `retrieval_candidates`, scoring |
| `context/memory_selection_tests.rs` | 704 | 13 tests for memory selection logic |
| `context/retrieval_provider.rs` | — | Retrieval pipeline: `RetrievalRequest`, `RetrievedMemoryCandidate` |
| `context/retrieval_view.rs` | — | Memory retrieval display rendering |
| `context/retrieved_memory_render.rs` | — | TUI rendering for retrieved memory cards |
| `context/compaction_view.rs` | — | Compaction progress rendering |
| `context/file_search_provider.rs` | — | File search integration for context |
| `context/mod.rs` | — | Module exports |

## Hook Subsystem

| File | Lines | Role |
|---|---|---|
| `hook_runtime.rs` | 321 | `HookRuntime`: event-bus subscriber, command hook executor, lifecycle dispatch |
| `hook_registry.rs` | 83 | Control-plane hook declarations: `HookRegistry`, `handle_control()` |
| `hooks.rs` | 275 | File-system hook discovery: `.claude/hooks/*.md` parsing, `discover_repo_hooks()` |

## Runtime Subsystem

| File | Lines | Role |
|---|---|---|
| `runtime_context.rs` | 700 | Builds `RuntimeBootstrap`: prompt config, tool manager, hook registry, embedding backend, event bus |
| `runtime_control.rs` | — | `RuntimeControlEvent`, `HookLifecycle`, `HookEvent` |

## Agent Subsystem

| File | Lines | Role |
|---|---|---|
| `agent.rs` | 1339 | Main agent loop, tool calling, streaming |
| `agent/compact/` | — | Context compaction pipeline |
| `agent/context_view.rs` | — | Agent context snapshot |
| `agent/control_handler.rs` | — | Control-plane event handling |
| `agent/memory_retrieval.rs` | — | Memory retrieval during agent turns |
| `agent/planning.rs` | — | Agent planning sub-loop |
| `agent/prompting.rs` | — | Prompt assembly for agent calls |

## LLM Subsystem

| File | Lines | Role |
|---|---|---|
| `llm.rs` | — | `LlmBackend` trait + provider dispatch |
| `llm/types.rs` | — | Request/response types |
| `llm/shared.rs` | — | Shared provider utilities |
| `llm/openai_compatible/` | — | OpenAI-compatible provider |
| `llm/gemini.rs` | — | Gemini provider |
| `llm/bedrock.rs` | — | AWS Bedrock provider |
| `llm/ollama.rs` | — | Ollama provider |

## Tools

| File | Lines | Role |
|---|---|---|
| `tools/bash.rs` | 2315 | Bash tool: execution, sandboxing, permission |
| `tools/agent.rs` | 1701 | Agent delegation tool |
| `tools/pty.rs` | 1649 | PTY terminal tool |
| `tools/` | — | Additional tools |

## TUI

| File | Lines | Role |
|---|---|---|
| `tui/state/types.rs` | — | `TuiApp`, `TranscriptEntry`, `RuntimePhase`, all state types |
| `tui/state/transcript.rs` | — | Transcript management: `push_entry`, `push_system` |
| `tui/render/cells/` | — | Cell renderers: active turn, committed turn, responding, message |
| `tui/runtime/events.rs` | — | `TuiEvent` dispatch |
| `tui/runtime/tasks.rs` | — | Async task completions: rebuild, compact, OAuth, models |

## Crates

| Crate | Role |
|---|---|
| `rara-memory` | Memory file operations + LanceDB vector database |
| `rara-persistence` | Persistent state DB (JSON) |
| `instructions` | Workspace memory + project instructions |
| `config` | Configuration types |
