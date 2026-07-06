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
OpenSSL for the aarch64 Linux target, and adds a pull-request `release-build`
workflow that builds, packages, and smoke-tests the same target matrix without
publishing release assets. The `aarch64-unknown-linux-gnu` cross image now also
installs the minimal build tools needed by vendored OpenSSL and protobuf-backed
build scripts.

## Validation

```bash
ruby -e 'require "yaml"; [".github/workflows/release.yml", ".github/workflows/release-build.yml"].each { |p| YAML.load_file(p); puts "#{p} ok" }'
cargo metadata --locked --format-version 1 --filter-platform aarch64-unknown-linux-gnu
cargo check --locked --bin rara
git diff --check
```
