# 2026-05-09 Spec Backlog Normalization

## Summary

Captured two backlog items that had implementation or planning material but no
stable feature-spec home:

- `support-acp` integration guidance for third-party ACP clients;
- `WorkspaceMemory` cache invalidation rules for prompt-source discovery.

## Decisions

- Keep `support-acp` as a repository skill for client authors, but define its
  stable contract in `docs/features/support-acp-integration.md`.
- Treat workspace memory cache invalidation as part of prompt-source discovery,
  not retrieval ranking or `MemorySelection`.
- Keep stable workspace memory near the stable prompt prefix while retrieval
  candidates remain in the volatile suffix.

## Validation

- Documentation-only change.
- `cargo fmt --check`
- `git diff --check`

## Follow-Up

- Add focused prompt-source discovery tests for cwd, branch, and nested
  workspace invalidation.
- Move appserver/ACP/Wire input handling to the shared runtime input-control
  bridge.
