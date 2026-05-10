# 2026-05-10 — AGENTS.md updated from AgentHub review

## What was done

Reviewed `/home/hawkingrei/devel/opensource/agenthub/AGENTS.md` and ported the
following rules to RARA's `AGENTS.md`:

1. **Fix obvious local issues** (Section 3 Architecture Constraints)
   - Unused imports, stale names, dead code, and compile warnings in the
     touched area should be fixed in the same change, not left for later.

2. **Strengthened Documentation Rules** (Section 5)
   - Changed "Non-trivial changes should update" to "Non-trivial changes MUST
     update".
   - Added: "Every non-trivial change must leave documentation" (matching
     AgentHub's "Every meaningful change should leave tracked documentation").
   - Added patterns for creating feature specs and journal notes.

## What remains

- Create `docs-feature` and `docs-journal` skills (like AgentHub's
  `agenthub-docs-spec` / `agenthub-docs-journal` skills) to automate spec and
  journal creation.
