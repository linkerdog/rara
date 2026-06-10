---
name: handle-unused-code
description: Handle unused code warnings by investigating purpose via journals, then choose the right fix: suppress, wire-up, or remove.
---

# Handle Unused Code

Use this skill when the compiler or clippy reports unused-code warnings
(`dead_code`, `unused_imports`, `unused_variables`, etc.).

## Core Rules

1. **Never blindly suppress or delete.**  First understand WHY the code was
   written before deciding what to do with it.
2. **Check the journal** (`docs/journal/`) and `docs/todo.md` for the item's
   purpose and planned activation milestone.
3. **Classify, then act.**  Every unused item falls into one of three buckets.

## Step 1 — Investigate

For each unused item:

1. Search the codebase for references (call sites, tests, config wiring).
2. Search `docs/journal/` for dated notes mentioning the symbol, its file,
   or the feature it belongs to.
3. Search `docs/todo.md` for the planned milestone.
4. Check git log (`git log --all -S "<symbol>"`) for the commit that introduced
   it, and read the commit message for intent.

## Step 2 — Classify

### Bucket A: Unimplemented capability

The code exists because a feature was planned but the implementation was
never finished.  The code describes the INTENT.

**Signs:**
- Feature-gated behind a cfg that is never active
- Part of a module that has TODOs or is mentioned in todo.md
- Constants, types, or functions that match a documented plan in a journal note
- Named after a feature or concept that is still listed as `[ ]` in docs/todo.md

**Action:** Suppress the warning with `#[allow(...)]` and a comment linking the
planned milestone.  Do NOT delete — the code serves as documentation and will
be activated when the feature is completed.

### Bucket B: Genuinely dead

The code was used once but later replaced, refactored out, or the caller was
removed.  It serves no current or planned purpose.

**Signs:**
- No references anywhere in the codebase or git history with meaningful callers
- Not mentioned in journals, todo.md, or feature specs
- The original caller was deleted or replaced at some point
- No feature gate that would make it live

**Action:** Delete.  Do not rename with underscore, do not `#[allow]`.
Remove the item and any now-unused imports it pulls in.

### Bucket C: Reserved namespace / palette

A group of related constants, enum variants, or config keys that form a
palette or register.  Most are used; a few are not yet wired.

**Signs:**
- A group of related items in the same module, all about the same concept
- Most are referenced elsewhere; one or two are not
- The unused items are documented in a journal note as placeholders

**Action:** Treat each unused item individually.  If it belongs to Bucket A
(unimplemented capability), suppress with an item-level `#[allow]` and a
comment.  If it belongs to Bucket B (genuinely dead), delete it.  **Never
use `#![allow(dead_code)]` at module level** — it hides genuinely dead
code alongside reserved items.

## Step 3 — Fix

### For Bucket A (unimplemented capability)

```rust
/// Reserved for consolidation index output.
/// Will be activated when Phase 2 merge is completed (docs/todo.md).
#[allow(dead_code)]
const INDEX_FILE: &str = "MEMORY.md";
```

The comment must:
- Say what the item is for
- Reference the todo.md or journal entry that tracks it
- Use "Reserved" or "Will be activated" language

### For Bucket B (genuinely dead)

Delete everything: the item, its imports (if now unused), related helper
functions, and tests.  One commit per logical group.

### For Bucket C (reserved palette)

Treat each unused item in the palette individually using the Bucket A or
Bucket B rules.  The palette as a whole is NOT a reason to blanket-suppress
all items.  Each item must stand on its own.

```rust
// If the item is reserved for future use:
/// Reserved hook level — will be activated with plugin system (docs/todo.md).
#[allow(dead_code)]
pub const PostToolUse: HookLevel = HookLevel::new("post_tool_use");

// If the item is genuinely dead (no plans, no references):
// Just delete it.
```

## Special cases

- **`#[cfg(feature = "tokio")]`** with no declared `tokio` feature: declare the
  feature in `Cargo.toml` (`[features] tokio = []`).  Do NOT remove the cfg
  gate if it guards planned async work.
- **Serde-only fields** (`#[serde(default)]` but never read): the field is
  for deserialization compatibility.  Keep it with `#[allow(dead_code)]`.
- **`unused_imports`** with conditional compilation: use `#[cfg(feature =
  "...")]` on the import, or inlay the import inside the feature-gated
  function.
- **Tests that don't compile because of API changes**: fix the test, don't
  suppress the warning.

## Workflow

```
compiler warning → identify unused item
                  → search docs/journal/ for purpose
                  → search todo.md for milestone
                  → classify as A, B, or C
                  → apply the bucket's action
                  → cargo check to verify warning is gone
                  → if bucket A or C: run cargo fmt, commit with
                    "chore: suppress unused <item> (reserved for <feature>)"
                  → if bucket B: run cargo fmt, commit with
                    "chore: remove dead code <item>"
```

## Example

```sh
# The compiler says:
warning: constant `INDEX_FILE` is never used
```

1. Search journal: `rg INDEX_FILE docs/journal/` → found in 2025-03-15-consolidation.md
2. Search todo: `rg INDEX_FILE docs/todo.md` → `- [ ] Phase 2 consolidation merge`
3. Classify: **Bucket A** — unimplemented capability
4. Action: `#[allow(dead_code)]` with comment `// Reserved for consolidation Phase 2 (docs/todo.md)`
