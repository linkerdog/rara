# Agent Definition Status Metadata

## Summary

RARA now applies Claude-compatible `hidden` and `description` metadata to the
repo-local agent definition listing used by `/status`.

## Key Decisions

- Keep `hidden` scoped to listing/status behavior for this checkpoint.
  Hidden definitions remain valid runtime definitions when directly resolved by
  `spawn_agent`.
- Include non-empty frontmatter `description` text in visible status lines so
  imported agent summaries are useful without opening the markdown file.
- Leave execution semantics for `token_budget` and `permission_mode` as the
  remaining `AgentDefinition` metadata work.

## Validation

```bash
cargo test agents_ext::tests -- --nocapture
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
git diff --check
```

## Follow-Ups

- Implement execution semantics for `token_budget` and `permission_mode`.
