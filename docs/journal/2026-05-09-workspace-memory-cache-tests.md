# 2026-05-09 Workspace Memory Cache Tests

## Summary

Closed the prompt-source discovery and workspace-memory cache invalidation
test gap after documenting the cache contract.

## Coverage

- Cwd changes inside a workspace keep nested instruction discovery active.
- Cwd changes outside a workspace fall back to the workspace root.
- Git `HEAD` changes invalidate cached branch information.
- Modified instruction files refresh cached prompt-source content.
- A local `memory.md` created after an initial discovery miss is picked up by a
  later prompt-source discovery pass and refreshes the memory availability
  cache.
- New cache-invalidation tests hold the shared cwd lock because prompt-source
  discovery reads the process current directory.

## Validation

- `cargo test -p rara-instructions workspace::tests`
- `cargo fmt --check`
- `git diff --check`
