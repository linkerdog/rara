# Release Assets And Debian Packages

## Summary

The `.github/workflows/release.yml` release workflow now stages Debian packages
for Linux targets and performs an explicit GitHub Release asset upload after the
release is created. This prevents a tag release from existing without binary
assets attached.

## Scope

- Package `.deb` files for Linux release targets using Debian archive names.
- Include `.deb` files in the release checksum set.
- Upload staged release files with `gh release upload --clobber` after
  `softprops/action-gh-release` creates or updates the release.
- Verify the final GitHub Release asset count.

## Release Matrix Follow-up

The first `v0.0.3` release run exposed target-specific failures that were not
covered before merge:

- macOS failed while installing `protoc` through an unauthenticated setup action
  that hit the GitHub API rate limit.
- Linux musl builds failed in `openssl-sys` because the current dependency graph
  expects target OpenSSL metadata that is not available in the musl runners.
- Windows builds failed because local model server process handling imported
  Unix-only `nix` modules.

The follow-up keeps the published Linux binaries on GNU targets
(`x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu`) built on
`ubuntu-22.04`, installs `protoc` through runner package managers, uses vendored
OpenSSL for the aarch64 Linux target, and drops `x86_64-apple-darwin` from the
release matrix because the current release scope does not require `macos-13`.
The `aarch64-unknown-linux-gnu` cross image now also installs the minimal build
tools needed by vendored OpenSSL and a fixed upstream `protoc` version for
protobuf-backed build scripts. The fixed `protoc` install is required because
the cross image's Ubuntu xenial package repository provides `protoc` 2.6.1,
which does not support `--experimental_allow_proto3_optional`.

The pull-request `release-build` trigger was removed after the validated targets
passed so normal pull-request CI does not run release packaging. The
`release-build` workflow remains as a post-merge `main` check to build, package,
and smoke-test the release target matrix without publishing assets.

## Validation

```bash
ruby -e 'require "yaml"; [".github/workflows/release.yml", ".github/workflows/release-build.yml"].each { |p| YAML.load_file(p); puts "#{p} ok" }'
cargo metadata --locked --format-version 1 --filter-platform aarch64-unknown-linux-gnu
cargo check --locked --bin rara
git diff --check
```

## v0.0.5 Release Count Fix

The first `v0.0.5` release run built every matrix target successfully, but the
`github-release` job failed while staging checksums because the release workflow
still expected eight binary assets. After `x86_64-apple-darwin` was removed from
the release matrix, the release produces seven binary assets: five target
archives and two Linux Debian packages.

The workflow now stores the expected binary asset count in
`RELEASE_BINARY_ASSET_COUNT` and derives the final GitHub Release asset count as
`binary_count * 2 + 1`, covering each binary asset, each `.sha256` file, and
`checksums.txt`.

Validation:

```bash
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts ".github/workflows/release.yml ok"'
bash -lc 'RELEASE_BINARY_ASSET_COUNT=7; expected=$((RELEASE_BINARY_ASSET_COUNT * 2 + 1)); test "$expected" = 15; echo "$expected"'
git diff --check
```

## User-Facing Archive Names

Release archives now use user-facing platform labels instead of full Rust target
triples. The build matrix still compiles with triples such as
`x86_64-unknown-linux-gnu`, but the uploaded archives are named:

- `rara-${VERSION}-macos-aarch64.tar.gz`
- `rara-${VERSION}-linux-x86_64.tar.gz`
- `rara-${VERSION}-linux-aarch64.tar.gz`
- `rara-${VERSION}-windows-x86_64.zip`
- `rara-${VERSION}-windows-aarch64.zip`
