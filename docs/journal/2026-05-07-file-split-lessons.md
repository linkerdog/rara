# File Split Lessons Learned

**Date**: 2026-05-07
**Context**: Split 11 oversized source files (>800 lines) into directory modules.
**Result**: One correct split (#265, `compact.rs`), 14 invalid `include!` PRs closed (#250-#263), one revert in progress (#268).

---

## What Went Wrong

### `include!` Is Not Splitting

```rust
// mod.rs
include!("main.rs");
```

This moves a file into a directory but keeps it as a single monolithic file. Zero structural change.

## Correct Pattern: Types Extraction

Successful split of `compact.rs` (#265):

| File | Lines | Content |
|------|-------|---------|
| `types.rs` | 116 | All struct/enum/const definitions, `pub(crate)` visibility |
| `main.rs` | ~1325 | `impl Agent` + helpers, `use crate::agent::*` replaces `use super::*` |
| `tests.rs` | ~140 | Test module |
| `mod.rs` | 13 | Submodule declarations + targeted re-exports |

### Required Visibility Changes

| Change | Why |
|--------|-----|
| `struct → pub(crate) struct` | Cross-submodule visibility |
| Fields → `pub(crate)` | Helpers need to read struct fields |
| `pub(super) → pub(crate)` | After split, `super` is the directory module, not the original parent |
| `use super::* → use crate::X::*` | Same reason |



## Pitfalls

1. **Line number drift**: inserting `use` statements shifts all subsequent line numbers. Use `git show` to verify boundaries against the original file.

2. **Struct boundary off-by-one**: `sed -n 'start,endp'` easily misses the closing `}`. Always visualize the extracted range before deleting:
   ```
   git show HEAD:src/file.rs | sed -n 'start,endp'
   ```

3. **Private Agent methods**: `push_history_message`, `replace_history`, `extend_history_messages` are `impl Agent` methods called from helpers within the same module. After splitting into submodules, they need `pub(crate)`.

4. **`#[cfg(test)]` constants**: If types.rs contains test-only constants, keep them there with `#[cfg(test)]` or replicate in tests.rs.

5. **Don't touch `impl` + helpers boundary**: The most fragile split point. Compact's agent_impl was 332 lines (safe) and helpers were 895 lines (slightly over). Leaving them together in main.rs avoided 15+ visibility errors.

## Recommended Workflow

```
1. Restore original from git: git show COMMIT^:path > file.rs
2. grep -n to find struct, fn, cfg(test) boundaries
3. sed with exact line ranges, visualize first
4. Add pub(crate) to types and struct fields
5. cargo check → fix errors iteratively
6. Verify each sub-file < 800 lines
7. One PR per file
```
