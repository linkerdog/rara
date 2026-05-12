# 2026-05-12 Main Sync Development Plan

## What Changed On Main

Synchronized `main` from `5a6f6ae` to `608986f`. The recent changes move RARA
forward in several areas:

- Claude Code plugin compatibility: new `rara-plugins` crate, command-hook
  parsing/execution, and an initial middleware bridge.
- Memory and prompt sources: auto-memory extraction and directory-walking
  `.rara/rules/*.md` prompt sources.
- Provider/model surface: Kimi catalog support, DeepSeek model-window metadata,
  provider labels, and connection-flow cleanup.
- TUI: bottom-pane state extraction, paste-burst handling, overlay scrolling,
  collapsible thinking display, and status formatting.
- Agent reliability: sub-agent limits/progress/timeout behavior, duplicate
  tool-call detection, and anti-spin prompt guidance.

## Plan Update

The next development plan should prioritize integration gaps over starting
another independent feature line:

1. Finish plugin runtime integration before expanding plugin feature scope.
   The crate exists, but runtime startup, blocking semantics, matcher
   evaluation, MCP launch integration, install/list/remove, and source
   visibility are still the real product boundary.
2. Complete provider/model catalog and API-list polish. The next visible slice
   is context-window display in ModelSearch plus provider API model listing
   with catalog fallback.
3. Continue TUI cleanup by completing the `BottomPaneModel` migration and then
   moving pending-interaction flows into a bottom-pane view stack.
4. Add observability and controls around auto-memory extraction before making it
   more aggressive.
5. Keep web/source-reporting, auxiliary-model routing, cross-process sub-agent
   reattach, Terminal-Bench readiness, and release packaging as follow-up lanes.

## Trade-offs

Plugin integration is now the highest-leverage lane because it intersects hooks,
MCP, skills, agents, permissions, and control-plane readiness. Starting prompt
hooks or marketplace features before command hooks are fully observable would
increase the permission and debugging surface too early.

Provider and TUI work remain important, but they are more incremental and can
be sliced after the plugin runtime has a working end-to-end path.

## Updated Artifacts

- `docs/todo.md`
- `docs/features/claude-plugin-runtime.md`
- `docs/features/provider-connection-redesign.md`
