# 2026-05-09 Protocol Prompt Source Provenance

## Summary

Closed the first control-plane gap for protocol-registered prompt sources:
registrations now retain the `RuntimeControlEnvelope` provenance instead of
storing only the prompt-source payload.

## Changes

- Added `PromptSourceRegistry::handle_control_with_provenance`.
- Added `ProtocolPromptSourceSnapshot` so future prompt-runtime integration can
  consume registration, lifetime, and provenance from one stable object.
- Updated `control_plane::dispatch` to pass envelope provenance into prompt
  source registration.
- Added a focused regression test for provenance retention.

## Remaining Work

Protocol prompt sources still need to be converted into prompt-runtime
`PromptSource` entries and surfaced through `/context` as active or available
prompt inputs. This follow-up should consume the snapshot object added here
instead of re-reading protocol registry internals.

## Validation

- `cargo test protocol_sources::tests::prompt_source_registry_preserves_control_plane_provenance`
- `cargo fmt --check`
- `git diff --check`
