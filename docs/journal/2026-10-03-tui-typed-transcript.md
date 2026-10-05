# Typed Transcript And TUI Source Boundaries

## Scope And References

Complete #928's remaining source/test boundaries and role typing after earlier
stack checkpoints split markdown/state/task helpers and made cell modules
facades. Preserve storage, runtime protocols, display labels, and snapshots.

Inspected Codex `ea2046f36d5ee12d39c8e168fc3e5129301afa2b`:
`tui/src/history_cell/{mod,messages}.rs` separates user/assistant message cell
responsibilities; its app events carry typed operations. Inspected Claude Code
`4b9d30f7953273e567a18eb819f4eddd45fcc877`, `src/utils/messages.ts`: assistant
message constructors assign the discriminant and provider role explicitly.
Adapt typed construction and responsibility-based modules without adopting
another renderer or changing stored transcripts.

## Plan

1. Split the remaining 1827-line active-cell tests by responsibility, preserving
   every assertion and the existing snapshot's module identity. Replace the
   remaining TUI source `include!` boundaries with actual private modules.
   Entry: source inventory and file-split lessons reviewed. Exit: unchanged
   test inventory, compile checks, and all effective modules below the limit.
2. Carry typed roles through transcript construction, rendering, and persistence
   conversion. Keep unknown stored labels explicit and sanitized. Replace the
   unused role-string runtime compatibility dispatch with typed presentation
   events; migrate its tests to the production structured event path.
   Entry: stable storage/display contract documented. Exit: typecheck rejects
   invalid live role literals and persistence/render regressions stay intact.
3. Audit the issue's unverified sequence-reset concern against actual bus/port
   ownership, add focused evidence where needed, and validate the integrated
   change. Entry: runtime ordering producers identified. Exit: contract-backed
   conclusion, strict checks, PR, and review/issue checkpoint.

All changes are internal and reversible; no public runtime API, persistence
format, database schema, or external protocol change is planned. Local source,
documentation, normal Git/PR writes, and existing default test targets suffice.

## Implementation

`TranscriptEntry`, live producers, progress grouping, interaction completion,
and cell renderers now carry `MessageRole`. Semantic matches enumerate variants.
Only persistence restoration converts stored labels; known spellings round-trip
and sanitized unknown labels retain a neutral `Legacy` representation. Storage,
provider roles, memory export, and public embedding contracts remain unchanged.
`DownloadProgress` and `OAuthProgress` replace arbitrary role-keyed events.
Assistant and tool semantics use the existing structured runtime reducer.

Migrating compatibility tests to real `AgentEvent` inputs exposed two existing
structured-reducer gaps: delegated JSON was passed to helpers requiring formatted
tool-name prefixes, and `bash` search actions were absent from the structured
exploration classifier. Tests now use actual delegated tool JSON and retain the
original assertions. A small private module decodes summaries and pending
questions directly, preserving both in one result. Failed/unrelated results do
not create questions; malformed delegated results remain visible and emit a
warning. Search classification reads the structured command argument.

The 45 active-cell tests moved into three responsibility-based modules (21
presentation, 17 stream/order, 7 tool/output), with the existing snapshot test
retaining its original module. The near-limit session restoration tests now have
a normal child module. Four source `include!` sites became ordinary modules for
status sections, task completion, and event tests. All TUI source files are below
1000 lines; cell `mod.rs` remains a facade. Superseded string parsers were removed
after checking their structured-events history (`5ea2103c`), journals, and TODOs.

## Sequence-Domain Audit

The issue's reset concern is not reproduced by the current in-process ownership:

- `RuntimeClient` initializes its event bus once from bootstrap. Backend rebuild
  completion replaces the agent/services without replacing the TUI's event bus.
- `InProcessRuntimeClientPort` retains a fixed `Arc<RuntimeEventBus>`; each
  subscription uses that bus. The controller retains its port for its lifetime.
- Publication sequences advance even with no subscribers. Per-request control
  events are resequenced on publication into this same bus.
- A reconnect notification does not clear the controller's session/turn fence.
  A regression now resubscribes to the actual port, retains the dedup cursor,
  accepts the later publication, and rejects replay of the first event.

Keep the watermark strict. A future transport replacing its ordering domain must
supply an explicit adapter/controller lifetime boundary; a lower sequence or a
reconnect notification is not proof of a new domain. No ordering behavior or
external protocol changed here.

## Validation

Observed regression failures before the structured-reducer fixes:

- Actual delegated JSON failed all three exploration/planning/worker question
  cases: missing summaries or no pending question.
- Structured `bash rg` inputs produced no exploration actions, failing the
  existing two-command expectation.

A temporary legacy-restoration mutation (`Legacy` fallback replaced by `System`)
failed the live-to-committed round-trip test at its unknown-label assertion.
Separately, changing a live producer to `push_entry("Agnet", ...)` was rejected
by Rust's `MessageRole` type boundary. These probes were restored before final
validation; the type failure is a compile-time guard, not behavioral RED evidence.
The restoration case checks known labels, escape sanitation, neutral display,
no accidental stream activation, persisted spelling, and a second restore.

- `cargo test --locked --lib tui::`: 931 passed; three ignored isolated child
  fixtures are exercised by their parent tests.
- `cargo test --locked --lib auto_memory::tests::`: five passed.
- `cargo test --locked --lib runtime_event_bus::tests::`: eleven passed,
  including per-request resequencing and concurrent publication ordering.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed without new warnings.
- `bazel test //:rara_unit_tests --test_arg=tui::`: 931 passed, the same three
  isolated fixtures skipped directly and exercised by parents. The final test
  run passed on its first attempt after dependency recovery.
- Test-name inventory retains all 45 moved cases; existing snapshots unchanged.
  Formatting, whitespace, source-size, and source-inclusion audits pass.

Initial default Bazel resolution encountered the recurring missing external
`rules_rust` cache package. A scoped force fetch restored dependencies; thread/process inspection located
three slow upstream Git fetches with growing pack files. No Bazel configuration
or BUILD files changed. The existing gold-linker warning remains unrelated to
this change. Local validation is complete; remote CI is a separate gate.

## Remaining Work

Exact-head remote CI, review, and eventual stack integration remain delivery
gates. Physical terminal acceptance (#925) and goal resume (#931) are separate
issues. Future remote ordering epochs belong to their adapter contract.
