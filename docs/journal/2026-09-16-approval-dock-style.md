# Approval Dock Surface Alignment

**Date**: 2026-09-16
**Scope**: Make the pending approval dock use the standard TUI bottom-pane
surface.

## What Changed

- Removed the full-width status-color background that appeared whenever a
  pending interaction was active.
- Render approval headings and shortcut markers as lightweight semantic section
  elements on the normal bottom-pane background.
- Keep the warning color for shell approvals and the accent color for planning
  interactions, including the selected action.

## Why

The former alert background made the permission decision surface visually
inconsistent with the transcript and other bottom-pane interactions. The new
presentation retains its safety cue without changing approval choices,
shortcuts, or policy behavior.

## Verification

- Focused render coverage asserts that a shell approval dock uses the standard
  bottom-pane background across its full rendered area.
