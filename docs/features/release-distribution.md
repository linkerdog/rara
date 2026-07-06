# Release Distribution

## Problem

RARA currently has CI coverage for formatting, build, clippy, and tests, but it
does not have a release pipeline that turns a version tag into downloadable
artifacts or downstream package updates.

Without a documented release contract, future Homebrew and npm distribution work
would likely grow as ad hoc workflow logic. The release path should be designed
around stable artifacts first, then package-manager adapters.

## Scope

- Define a tag-driven release workflow for the `rara` binary.
- Define the release artifact naming and platform matrix.
- Define how GitHub Releases, Homebrew, and npm should connect to the same
  canonical artifacts.
- Define validation gates before publishing.

## Non-Goals

- Publish Homebrew formulae or npm packages in this phase.
- Add install scripts or binary auto-update behavior in this phase.
- Replace existing build, fmt, clippy, or test CI jobs.

## Architecture

The release pipeline should have four layers:

1. **Tag validation.** A release starts from a pushed `vX.Y.Z` tag. Pre-release
   tags use `vX.Y.Z-alpha.N` or `vX.Y.Z-beta.N`.
2. **Matrix build.** Build one binary per supported target with the pinned Rust
   toolchain from `rust-toolchain.toml`.
3. **Artifact staging.** Package binaries into deterministic archives and expose
   checksums.
4. **Distribution adapters.** Publish GitHub Release assets first. Homebrew and
   npm consume those assets instead of rebuilding from source.

This keeps GitHub Release assets as the source of truth. Homebrew and npm are
thin distribution layers over the same build outputs.

## Target Matrix

Initial release targets:

| Target | Runner | Archive |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-14` | `.tar.gz` |
| `x86_64-apple-darwin` | `macos-13` | `.tar.gz` |
| `x86_64-unknown-linux-musl` | `ubuntu-latest` | `.tar.gz`, `.deb` |
| `aarch64-unknown-linux-musl` | `ubuntu-latest` with `cross` | `.tar.gz`, `.deb` |
| `x86_64-pc-windows-msvc` | `windows-latest` | `.zip` |
| `aarch64-pc-windows-msvc` | `windows-latest` | `.zip` |

The matrix can be narrowed before the first public release if local dependency
or linker behavior makes a target unreliable.

## Artifact Contract

Each build produces one executable archive:

- Unix: `rara-${VERSION}-${TARGET}.tar.gz`
- Windows: `rara-${VERSION}-${TARGET}.zip`
- Debian package for Linux targets: `rara_${VERSION}_${DEB_ARCH}.deb`

Each release should also publish checksum files:

- `rara-${VERSION}-${TARGET}.sha256`
- `checksums.txt`

Archives contain only the executable at the archive root:

- `rara`
- `rara.exe`

Debian packages install the executable to:

- `/usr/bin/rara`

Debian package architecture names follow Debian conventions:

- `amd64` for `x86_64-unknown-linux-musl`
- `arm64` for `aarch64-unknown-linux-musl`

## GitHub Release Contract

The GitHub Release job should:

- download all matrix build artifacts;
- verify expected target coverage;
- generate checksums;
- create or update the release for the tag;
- mark versions containing `-` as pre-releases;
- attach all binary archives and checksums;
- use generated release notes.

GitHub Release publishing is the first distribution gate. Downstream publish
jobs should not run unless the release assets exist.

## Current Pipeline

Pull-request CI includes:

- Linux build, fmt, clippy, and test jobs on `ubuntu-latest`;

Post-merge CI additionally includes a Windows build gate on `windows-latest`
using the pinned Rust toolchain and `cargo build --locked`.

Windows tests are intentionally not part of the first post-merge CI slice. The
release workflow still owns Windows archive packaging and native smoke tests.
SQLite is built through `rusqlite`'s bundled feature so Windows builds do not
depend on a runner-provided `sqlite3.lib`.

`.github/workflows/release.yml` implements the first release slice:

- tag validation for `vX.Y.Z`, `vX.Y.Z-alpha(.N)`, and `vX.Y.Z-beta(.N)`;
- explicit tag checkout for both tag push and manual dispatch releases;
- Cargo package version validation against the tag version;
- pinned Rust toolchain discovery from `rust-toolchain.toml`;
- target matrix release builds;
- deterministic binary archives;
- Debian packages for Linux `amd64` and `arm64` targets;
- runnable archive smoke tests on native runner/target pairs;
- checksum generation;
- GitHub Release creation through the release action, followed by explicit
  `gh release upload --clobber` asset publishing and verification.

It intentionally does not publish Homebrew or npm packages yet.

## Homebrew Adapter

Homebrew should be a follow-up adapter, not the first release mechanism.

The adapter should:

- read `checksums.txt` from the GitHub Release;
- update a formula in a tap repository;
- map `darwin-aarch64` and `darwin-x86_64` archives to the same formula;
- avoid publishing pre-release tags unless a separate tap policy is defined.

Open design choice:

- use a dedicated tap repository such as `linkerdog/homebrew-tap`, or keep a
  formula under this repository until the release process stabilizes.

## npm Adapter

npm should use a meta-package plus platform packages:

- `rara`: JavaScript wrapper and platform selection logic.
- `@rara/darwin-arm64`
- `@rara/darwin-x64`
- `@rara/linux-x64`
- `@rara/linux-arm64`
- `@rara/win32-x64`
- `@rara/win32-arm64`

The npm staging script should:

- consume GitHub Release archives;
- generate platform package tarballs;
- keep the top-level package small;
- run package tests before publishing;
- use npm trusted publishing through OIDC.

Stable tags publish to `latest`. Pre-release tags can publish to `alpha` or
`beta` only when the tag format explicitly matches that channel.

## Validation Matrix

Before creating release assets:

- `cargo fmt --check`
- `cargo build --locked --release --target <target>`
- `cargo test --locked` on at least the host target
- archive smoke test: unpack and run `rara --version`
- checksum verification

Before npm publishing:

- packaging script unit tests
- unpacked package smoke test
- platform package resolution test

Before Homebrew publishing:

- formula syntax audit
- install test on supported macOS runners when practical

## Open Risks

- `rara` currently has a large dependency graph. Release builds may expose
  platform-specific linker or cross-compilation failures.
- Local model backends and native dependencies may require target-specific
  feature decisions before binary distribution.
- npm package ownership, scope naming, and trusted publishing setup are external
  operational prerequisites.
- Homebrew tap ownership and formula update permissions are external
  operational prerequisites.

## Source Journals

- `docs/journal/2026-05-06-release-distribution-planning.md`
