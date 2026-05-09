# Retrieval Hook And Graph Providers

RARA now reserves explicit retrieval-provider slots for hook output and graph
context.

## What Changed

- Added precomputed hook-output and graph-context providers to the
  `RetrievalSourceProvider` boundary.
- Threaded hook-output and graph-context candidate vectors through
  `RuntimeContextInputs` and `Agent`.
- Added `/context` source-status rows for hook output and graph context.
- Kept both source classes non-injected until hook execution policy and graph
  confidence policy are explicit.
- Updated retrieval orchestration tests to fix provider ordering and source
  visibility.

## Cache And Context Placement

Hook and graph candidates are volatile retrieval suffix material. They do not
belong before system prompt, tools, stable skills, project memory, or compacted
history. The current slice only makes them observable and rankable through the
same candidate contract used by memory, file search, and MCP resources.

## Validation

- `cargo fmt --check`
- `git diff --check`
- `cargo metadata --locked --no-deps`
- `CARGO_TARGET_DIR=/private/tmp/rara-check-target RUSTFLAGS=-Cdebuginfo=0 cargo check --locked -p rara --bin rara`

Targeted `cargo test --locked retrieval_provider` was attempted with the same
temporary target directory and reduced debuginfo, but the local linker stopped
with `ld: write() failed, errno=28 (No space left on device)`. CI should
validate the focused tests in a clean runner.
