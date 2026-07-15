# RARA 0.0.13 Version Bump

## Summary

RARA's workspace package version is now `0.0.13`, the next patch release after
`0.0.12`.

## Scope

The Cargo workspace manifest and lockfile carry the release version. This
change prepares the package version for a later release tag; it does not create
or push a tag.

## Validation

- `cargo metadata --locked --no-deps --format-version 1`
- package version check for `rara` at `0.0.13`
- `cargo fmt --check`
- `cargo check`
