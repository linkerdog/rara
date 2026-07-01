# Clippy CI Warning Gate

## Summary

The clippy workflow now runs the full workspace with warnings denied:

```bash
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
```

This makes lint warnings fail CI instead of being advisory output.

## Cleanup Scope

The accompanying cleanup keeps behavior unchanged:

- mechanical clippy simplifications such as collapsed conditionals, direct
  `Result` matches, `Default` implementation, and iterator rewrites;
- explicit parameter structs where they remove ambiguous positional arguments;
- narrowly scoped `#[allow(clippy::...)]` annotations only on protocol,
  persistence, or runtime composition boundaries where reshaping the API would
  create churn without improving correctness.

## Validation

```bash
cargo fmt
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings
```
