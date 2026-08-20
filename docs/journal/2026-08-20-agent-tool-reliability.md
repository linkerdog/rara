# Agent Tool Reliability

## What changed

RARA now closes the three reliability gaps reported in
[issue #829](https://github.com/linkerdog/rara/issues/829):

- the macOS Seatbelt profile permits ordinary runtime reads while preserving
  write containment, network isolation, and explicit sensitive-root read
  denies;
- foreground shell results preserve Unix signal metadata and expose typed,
  evidence-backed sandbox failures instead of collapsing them into an unknown
  exit status;
- per-request tool-result projection distinguishes the active user turn from
  prior turns and replaces oversized results with tool-aware evidence summaries
  or references rather than a generic cleared marker;
- the LSP manager uses asynchronous JSON-RPC routing, shared lazy startup,
  bounded retry, document versions, cached diagnostic freshness, and typed
  failure/status objects.

The large `tool_result.rs` and `lsp_manager.rs` implementations were split into
private focused modules so the changed orchestration remains reviewable and no
production source file crosses the repository size limit.

## Why

The previous Seatbelt profile could terminate even read-only repository
inspection before output was produced. The nullable process exit code then hid
whether the process exited, received a signal, or encountered a policy denial.

Tool-result microcompaction treated protocol `tool_result` messages as user
turn boundaries and could remove evidence requested in the current turn. That
made the model repeat reads and searches despite the durable transcript still
containing the original result.

The previous LSP bridge mixed blocking process I/O, an unkeyed response
channel, repeated `didOpen` notifications, a fixed sleep, and executable probes
inside status reads. A delayed or early-exiting server therefore appeared only
as `running: false` or a generic wait error.

## Reference patterns

The implementation adapts, rather than copies, current upstream patterns:

- current Codex uses broad runtime reads plus explicit unreadable roots and
  reports process termination separately from policy classification;
- current Claude Code exposes asynchronous language-server lifecycle phases,
  preserves initialization failures, and rejects stale diagnostic versions;
- OpenCode `dev` at `b155b15694dbcc6768f11d2f25cc2bdd1f738ab4`
  shares in-flight server startup, applies a bounded initialize deadline, and
  explicitly answers workspace configuration, workspace folder, progress, and
  capability-registration requests while initialization is pending;
- the reviewed runtimes preserve compacted tool-result identity and useful evidence
  instead of treating the persisted transcript as disposable context.

RARA keeps these patterns behind its existing session-scoped manager, tool
result, context-observability, and TUI contracts.

## Design decisions

### Sandbox and process outcomes

Filesystem policy now follows a read-deny/write-allow split. Ordinary reads are
available to shells, Git, Cargo, dynamic loaders, and installed toolchains;
`~/.ssh` and `~/.aws` remain explicit read denies. Writes remain limited to the
workspace and temporary roots. Sensitive rules cover both configured and
canonical paths so macOS aliases such as `/var` to `/private/var` cannot bypass
them. Temporary writes include macOS's canonical `/private/tmp`.
`/dev/null` is explicitly available because system Git/Xcode helpers require
it even for read-only operations.

`termination` is derived directly from the OS status. `policy_denied` is added
only when captured output contains denial evidence. A signal without denial
evidence is reported as `sandboxed_process_signaled`, which preserves the
observation without overstating its cause.

### Tool-result projection

The latest real user-text message defines the active turn; protocol-only
`tool_result` messages do not. Projection applies pressure in this order:

1. prior-turn results become bounded references;
2. older active-turn results become bounded semantic head/tail summaries;
3. non-recent results become minimal references only if the budget still does
   not fit.

Recent results remain verbatim, tool-use/result pairing stays intact, and the
source transcript is never mutated. Observability now separates summarized,
reference-only, active-turn-kept, and legacy-cleared counts.

### LSP lifecycle

Each detected language server has a serialized startup gate and an explicit
`NotStarted`, `Starting`, `Ready`, `Unavailable`, or `Failed` state. JSON-RPC
responses are routed by request ID, while a supervisor races initialization
against process exit and timeout and retains a bounded stderr tail.

The first file synchronization sends `didOpen`; later calls send versioned
`didChange`. The protocol writer also answers the server-to-client control
requests used by current OpenCode and real language servers, including requests
that arrive before the initialize response. The tool returns immediately with
`current`, `cached`, or `pending` diagnostics instead of sleeping. Status
rendering reads cached state only and cannot spawn a process.

## Trade-offs

- Read access is intentionally broader on macOS, so sensitive roots must stay
  explicit final denies. Additional credential roots should be added through
  the same deny mechanism when they become part of the sandbox contract.
- Diagnostic delivery remains push-based. Servers that only support pull
  diagnostics need a future capability-specific adapter.
- Server-initiated feature requests outside the implemented control methods
  receive JSON-RPC method-not-found; RARA does not advertise those capabilities.
- Background task records retain their existing nullable exit code; the new
  structured termination object applies to foreground results in this change.

## Verification

- `cargo fmt --all -- --check`
- `cargo check --bin rara`
- `cargo test -p rara-sandbox`
- `cargo test tools::bash::outcome::tests --bin rara`
- `cargo test sandbox_policy_denial_is_machine_readable --bin rara`
- `cargo test foreground_signal_is_reported_as_structured_termination --bin rara`
- `cargo test tool_result::tests --bin rara`
- `cargo test agent::tests::microcompact::model_request_projects_old_tool_results_without_mutating_history --bin rara`
- `cargo test lsp_manager:: --bin rara`
- `cargo test tui::render::cells::lsp_diagnostics::tests --bin rara`
- `cargo test formats_signaled_bash_tool_result_without_unknown_status --bin rara`
- `cargo test runtime_control::tests --bin rara`
- `cargo test --bin rara --quiet`
- `cargo clippy -p rara-sandbox --all-targets -- -D warnings`
- `cargo clippy --bin rara --tests -- -D warnings`
- `git diff --check`

The macOS sandbox smoke test executes every command class from the issue:
`echo`, `date`, `ls`, `git log`, `git branch`, `git status`, `git diff`, and
`cargo fmt -- --check`. A separate end-to-end test attempts an out-of-workspace
write and verifies both containment and `sandbox_failure.kind = policy_denied`.

## Remaining work

No remaining work is required for issue #829. The capability-specific LSP and
background-task extensions above remain outside this checkpoint rather than
being added to `docs/todo.md` as active follow-up work.
