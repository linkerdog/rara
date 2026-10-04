# Runtime Main Integration

## Scope

Synchronize the lightweight session runtime with main after its approval-results
parent merged. A normal merge preserves the published branch history. Duplicate
crate additions from the earlier stack require explicit resolution because the
parent changes landed through squash merges.

## Decisions

Retain main's browser-local model/tool/loop effects, browser accounting clocks,
resolver 3, and root default-member selection. Add the runtime's shared history
repair and observation exports alongside those contracts. Native session
ownership, cancellation-return barriers, approval adapters, and replay continue
to use the extracted actor. The native-only host runtime does not imply browser
session scheduling or a browser provider transport.

The core Git consumer continues to audit native and browser graphs separately.
The runtime Git consumer exercises the public builder and the same host fixture
used in the workspace. The two consumers complement one another; they do not
inherit workspace patches or its lockfile.

## Validation

- Formatting, strict workspace/all-target Clippy, and browser all-target checks
  pass with the locked dependencies.
- Native checks pass: 13 core, 35 agent, 6 public host, 12 session, 11 event-bus,
  21 tool-result, 43 planning/approval, and 9 session/embedding integration tests.
- Headless Chrome passes the 3 shared-effect and 2 accounting tests.
- Default Bazel dependency generation succeeds. Its lock changes only runtime
  crate mappings, target selection, and manifest/lock input records; it adds no
  external repositories. The local default build stops before compilation
  because the existing external cache lacks `platforms//host` package files.
  The exact-head remote default Bazel build/test remains a delivery gate.
- Fresh Git core/runtime consumers run after the immutable revision is pushed
  and independently verified. The PR records that full SHA and their results;
  the previously published stacked revision is not evidence for this merge.

## Remaining Work

The runtime PR targets main after publishing and verifying its exact head.
The existing [downstream contract](../features/downstream-runtime.md) defines
the acceptance gate. Provider/context extraction, browser HTTP/SSE, and browser
session scheduling remain separate work under #871.
