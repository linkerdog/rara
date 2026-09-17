# TUI Interaction Specifications

This directory is the canonical home for user interaction contracts. Runtime,
provider, persistence, and protocol contracts remain in [features](../features/README.md).
Implementation checkpoints and reference comparisons belong in [journal](../journal/).

## Specifications

| Specification | Owns |
| --- | --- |
| [Commands](commands.md) | Discovery, canonical names, aliases, execution, and help |
| [Composer and overlays](composer-and-overlays.md) | Input ownership, key priority, search, selection, and dismissal |
| [Runtime feedback](runtime-feedback.md) | Running turns, queued input, cancellation, approvals, and recovery |
| [Quality verification](quality-verification.md) | Observable contracts, regression evidence, rendering, and CI boundaries |

## Reading And Change Rules

- Contracts describe the intended behavior of the current implementation.
  Known implementation gaps are explicitly listed under Open Risks; proposals
  are not silently promoted to implemented contracts.
- Each contract has a stable ID for test comments, issue descriptions, and
  review evidence. Keep IDs stable when moving text between files.
- For an interaction change, update the owning specification first, add the
  narrowest useful regression check, and record the checkpoint in a journal.
- Define the trigger, relevant state, state transition, visible result, and
  cancellation/error behavior. A screenshot alone cannot define the contract.
- A runtime-facing interaction references the owning feature contract rather
  than redefining authorization, provider configuration, or persistence here.

## Scope And Non-Goals

The initial specifications cover the existing terminal composer and built-in
TUI control surfaces. They do not define new CLI subcommands, a new runtime
protocol, full Vim editing, a plugin command platform, or browser UI behavior.

## Implementation Checkpoint

- [2026-09-17: TUI interaction contracts](../journal/2026-09-17-tui-interaction-contracts.md)
