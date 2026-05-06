# Feature Docs Standard

This directory stores stable, domain-oriented technical specifications.
Chronological implementation records belong in `docs/journal/`.

## Required Structure

Each active feature spec should include:

- `Problem`
- `Scope`
- `Non-Goals`
- `Architecture`
- `Contracts`
- `Validation Matrix`
- `Open Risks`
- `Source Journals`

## File Policy

- `docs/features/`: stable theme/domain docs only
- `docs/journal/`: date-prefixed implementation records
- When a feature evolves, update the canonical feature doc and add or append a journal note.

## Current Persistence Specs

- `session-transcript.md`: typed session and sub-agent transcript storage.

## Control-Plane Readiness

Features that affect skills, memory, prompt sources, hooks, planning, approvals,
tool output, `/context`, or `/status` must describe how the behavior can be
driven through the runtime control plane. Local TUI behavior should be one
adapter over the same structured request/event contract that ACP, Wire, and
future appserver integrations can use.

## Security And Approval Specs

- `shell-approval-policy.md`: bash read-only classification and reusable prefix
  approval boundaries.

## Runtime Extension Specs

- `mcp-runtime.md`: source-aware MCP configuration, registry, status, refresh,
  reconnect, resource, and Tool Search contracts.
- `bedrock-backend.md`: Bedrock SDK backend crate boundary and RARA adapter
  contract.
- `file-search.md`: shared gitignore-aware file discovery and fuzzy path
  ranking crate for tools, TUI pickers, and context routing.

## Release Specs

- `release-distribution.md`: tag-driven binary release, GitHub Release assets,
  Homebrew adapter, and npm adapter contracts.
