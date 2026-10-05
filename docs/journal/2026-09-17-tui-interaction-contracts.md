# TUI Interaction Contracts And Quality Pilot

## Summary

Define a dedicated [interaction specification set](../interaction/README.md),
reduce redundant command discovery, and protect concrete keyboard/selection
defects through the production dispatcher and renderer.

## Source Review And Evidence Boundary

The local implementation baseline is `09b049a9`. Read-only reference review:

- Codex remote HEAD `16f59db96eaa260eb2863974e4e6dea6bcca6bb3`:
  [command popup](https://github.com/openai/codex/blob/16f59db96eaa260eb2863974e4e6dea6bcca6bb3/codex-rs/tui/src/bottom_pane/command_popup.rs)
  hides duplicate aliases in default discovery;
  [slash commands](https://github.com/openai/codex/blob/16f59db96eaa260eb2863974e4e6dea6bcca6bb3/codex-rs/tui/src/slash_command.rs)
  encode task-time availability separately. Both files were retrieved at the
  remote revision, rather than relying on the older local checkout.
- OpenCode remote HEAD `88c6c7abc7f320b6aabed2634ac0b2d6e6ecea67`:
  [command palette](https://github.com/anomalyco/opencode/blob/88c6c7abc7f320b6aabed2634ac0b2d6e6ecea67/packages/tui/src/component/command-palette.tsx)
  derives reachable/visible commands from the keymap;
  [autocomplete](https://github.com/anomalyco/opencode/blob/88c6c7abc7f320b6aabed2634ac0b2d6e6ecea67/packages/tui/src/component/prompt/autocomplete.tsx)
  binds selection within the active input surface. Official
  [TUI](https://opencode.ai/docs/tui/) and
  [keybinding](https://opencode.ai/docs/keybinds/) documentation was also checked.
- The local Claude Code reference is a third-party reconstructed checkout,
  `instructkr/claude-code@4b9d30f`, not official upstream source. Its
  `src/commands.ts` separates availability and aliases. Treat it only as a
  design reference, supplemented by official
  [interactive-mode documentation](https://code.claude.com/docs/en/interactive-mode).
- Raft product snapshot: `slock@36d5cb976611836b7fffbea7784b6260833a117b`.
  `SOPs/test.md` requires a named contract, terminal oracle, and production-path
  RED proof; `packages/web/scripts/check-color-tokens.mjs` rejects new raw colors
  and stale baseline entries. RFC 037 separates transport, reducers, projections,
  and rendering but remains an incremental architecture proposal. Its test
  workflow places full E2E on staging/non-PR runs. Remote lookup returned
  Repository not found; these are local source findings, not current remote or
  deployed acceptance claims.

## Decisions And Bounded Plan

1. Establish observable contracts before edits. Exit when command inventory,
   key precedence, selection semantics, and known gaps are source-backed.
2. Remove duplicate discovery entries while retaining typed aliases. This
   reduces noise without breaking familiar spellings. Remove the unreachable
   Dream command residue rather than exposing another unfinished entry point.
3. Fix search input ownership, shared model filtering, and command invocation
   identity. Keep provider/runtime protocols and persisted formats unchanged.
4. Add regression checks at real key-dispatch/render boundaries. Exit with
   explicit validation results or a precise toolchain/dependency blocker.
5. Keep broader command availability, terminal acceptance, and quality ratchets
   in TODO with explicit evidence requirements rather than claiming them done.

Local source/docs writes, Cargo verification, and creating a PR are authorized.
No peer repositories are modified; no worktree or Bazel configuration change is
needed. The existing root Cargo/Bazel test registration includes the new module.

## Why These Changes

- `/runtime`, `/memory`, and `/threads` duplicate existing actions.
- `general_help_text` advertised removed auth commands, while the existing
  help test asserted against a separate test-only formatter.
- Searchable overlays consumed j/k as navigation, preventing literal input.
- Selecting `/tasks [task_list_id]` submitted the display placeholder as data.
- Model search rendering matched model labels only; navigation and execution
  also matched provider labels, making invisible selections possible.
- Model search changed local configuration without requesting runtime rebuild,
  and model identity omitted the endpoint/profile ID. Search now uses the same
  setup/runtime action path as the existing unified picker.
- The skills toggle only mutated presentation fields. A runtime snapshot could
  overwrite it without changing skill policy; a read-only inspector is the
  honest local boundary until runtime-owned enablement exists. The inspector
  now derives its label directly from the runtime invocation flag rather than
  maintaining a second, inverted boolean.
- Help Commands could display only the first viewport because Up/Down did not
  move the selected entry.

The transferable quality pattern is contract -> cheapest complete oracle ->
known-bad production behavior -> correct CI stage. React-specific tooling is
not appropriate for the Rust TUI. Existing semantic theme tokens, typed runtime
events, and the scripted harness are the foundation to extend.

## Validation

- Old production code at `09b049a9`, with the new test module/harness: 12 focused
  cases, 10 behavioral failures and 2 passing compatibility/empty-result cases.
- Corrected production code: the same 12 focused cases passed. The command is
  `cargo test --locked --lib tui::interaction_tests`.
- Representative RED signatures: command input `/sills` instead of `/skills`,
  model query empty instead of `jk`, no `Maintenance(Rebuild)` request, wrong
  endpoint identity, missing provider-filtered rows, and task-list placeholder
  submission. Help and skill tests inspect the production-rendered screen.
- Replaced test-only help/recommendation/normalization helpers and the skills
  fixture-only assertion with production-path checks. The unreachable Dream
  command test was removed with its implementation.
- The initial broader TUI run found one stale skills checkbox assertion and a
  leftover toggle hint. Both were corrected. The 1,464-line renderer test file
  was split into two modules below 1,000 lines, preserving snapshot identities
  and the moved test bodies unchanged.
- A fixture compilation error was corrected before the RED run; it was not
  counted as behavioral evidence.

Final local verification:

| Check | Result |
| --- | --- |
| `cargo test --locked --lib tui::` | 613 passed, 0 failed; includes all 12 new interaction cases |
| `cargo check --locked` | Passed |
| `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings` | Passed without warnings |
| `cargo fmt --all -- --check` and `git diff --check` | Passed |
| Changed documentation link targets | No missing targets |
| Changed/new Rust file sizes and renderer test move | All below 1,000 lines; moved bodies unchanged |

The macOS test linker reports the pre-existing `__eh_frame section too large`
warning, also present in the unchanged-production RED run. There are no new
Rust source warnings. No snapshot content was updated. Bazel, remote CI,
PTY/terminal restoration, and live-provider acceptance are not established by
these local checks.

## Follow-Ups

See the active TUI entries in [TODO](../todo.md) and the explicit Open Risks in
the interaction specs. They cover busy-time command availability, text editing
and draft restoration, narrow/Unicode rendering, event interleavings, style
ratchets, and PTY acceptance. None is claimed as an installed gate by this pilot.
