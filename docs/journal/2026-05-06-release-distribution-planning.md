# Release Distribution Planning

## Context

Reviewed a tag-driven Rust CLI release workflow that builds target-specific
binaries, stages GitHub Release assets, and publishes npm packages from staged
tarballs.

RARA currently has CI workflows for build, clippy, fmt, and test, but no release
workflow or package distribution contract.

## Extracted Pattern

- Validate release tags before doing expensive work.
- Read the pinned Rust toolchain from `rust-toolchain.toml`.
- Build a platform matrix instead of relying on one host build.
- Package binaries into deterministic archives.
- Upload build artifacts from matrix jobs and assemble the GitHub Release in a
  separate job.
- Treat GitHub Release assets as canonical.
- Run package staging and package tests before publishing npm artifacts.
- Use npm trusted publishing instead of long-lived npm tokens.
- Publish pre-release channels only for explicit pre-release tag patterns.

## RARA Decision

RARA should first define the release artifact contract, then add distribution
adapters.

The first implementation slice should add a GitHub Release workflow that only
builds and uploads binary archives plus checksums. Homebrew and npm should be
follow-up adapters that consume those release assets.

## Implementation Checkpoint

Added `.github/workflows/release.yml` for the first release slice:

- validate tags and Cargo package version;
- checkout the validated tag before building release artifacts;
- build six target archives;
- smoke-test runnable native archives with `rara --version`;
- generate per-archive and aggregate checksums;
- publish GitHub Release assets.

Also enabled the `rara --version` CLI flag so release smoke tests can run
without entering the TUI.

## Follow-Up

- Add npm package layout and staging tests.
- Add Homebrew tap update strategy after release assets are stable.
