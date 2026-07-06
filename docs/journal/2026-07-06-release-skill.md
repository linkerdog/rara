# Release Skill

## Summary

Added a repo-local `release-rara` skill so future releases follow a fixed
agent-assisted checklist before tags are created or workflow dispatches run.

## Background

The release workflow correctly rejects a tag when the tag version does not match
the Cargo package version. That guard prevents publishing mismatched binaries,
but the failure still happens late if a tag is created before the version bump.

## Scope

- Added `.agents/skills/release-rara/SKILL.md`.
- Registered the skill in `AGENTS.md`.
- Tightened the release workflow's Cargo version lookup to select the `rara`
  package by name.
- Expanded the release workflow mismatch error with the required recovery
  sequence.

## Validation

```bash
cargo fmt
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts ".github/workflows/release.yml ok"'
git diff --check
```
