# Bedrock Backend Crate

## Context

The Bedrock SDK backend was still implemented directly inside the main crate.
After adding separate crates for other runtime boundaries, the Bedrock provider
was a good candidate for the same structure.

## Implementation Checkpoint

- Added `crates/bedrock` as `rara-bedrock`.
- Moved AWS SDK Converse client construction and Bedrock-native request/response
  conversion into the new crate.
- Kept `src/llm/bedrock.rs` as a thin RARA adapter implementing `LlmBackend`.
- Removed direct AWS SDK dependencies from the main crate dependency list.
- Added focused tests for provider conversion and the main adapter mapping.

## Validation

- `cargo fmt --check`
- `cargo test -p rara-bedrock -- --nocapture`
- `cargo test llm::bedrock -- --nocapture`
- `cargo check`

## Follow-Up

- Add explicit `aws_profile` support to Bedrock configuration.
- Surface Bedrock region/profile/model in `/status` once provider details are
  generalized.
- Revisit a shared runtime-types crate if more provider crates need to implement
  backend contracts directly.
