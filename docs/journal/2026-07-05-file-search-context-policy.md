# File Search Context Policy

## What Changed

RARA now treats file search as a shared backend with two explicit consumers:

- explicit user-facing file discovery, including TUI file suggestions and
  `list_files`;
- optional automatic retrieval candidates controlled by `context_file_search`.

The default `context_file_search = "paths_only"` policy keeps fuzzy file-path
matches visible to `MemorySelection` and `/context`, but the candidates carry
only path and provenance metadata. `context_file_search = "off"` disables only
the automatic retrieval bridge.

## Why

The previous context-file-routing plan left two behaviors easy to conflate:
using `rara-file-search` for explicit file selection, and letting fuzzy matches
enter automatic context retrieval. Keeping both is useful, but automatic
retrieval must not imply that file contents were read or injected.

The paths-only policy preserves observability and ranking experiments while
leaving content access behind explicit tools such as `read_file`.

## Trade-offs

- File-search candidates can still appear in `/context`, but they are
  intentionally low priority.
- Token budget estimates charge only the path/provenance manifest, not file
  content.
- The policy is coarse-grained for now: automatic routing is either disabled or
  paths-only.

## Verification

- Config tests cover default omission and explicit `off` deserialization.
- File-search provider tests cover paths-only budget estimation and retrieval
  candidate metadata.

## Remaining Work

No open follow-up is required for the two-track policy. Future explicit content
search should remain separate from automatic file-content injection.
