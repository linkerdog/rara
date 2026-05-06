# File Search Crate

Introduced `crates/file-search` as the shared workspace file-discovery layer.

Implemented:

- gitignore-aware traversal through `ignore`;
- fuzzy path ranking through `nucleo`;
- bounded file listing with total count and truncation metadata;
- stable ordering for list results and fuzzy score tie-breaks;
- `list_files` tool adapter using the new crate while preserving RARA's
  build-artifact suppression policy.

Validation:

- `cargo test -p rara-file-search -- --nocapture`
- `cargo test tools::file::tests::list_files_skips_build_artifacts_by_default -- --nocapture`

Observed existing noise:

- root crate tests still emit unrelated dead-code warnings;
- macOS linker warned that the `__eh_frame` section is too large during the
  focused root test link.
