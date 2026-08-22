# Terminal-Bench Headless Runtime Profile

## Summary

RARA now exposes a versioned `headless-coding-v1` runtime composition through
both `RuntimeSessionBuilder` and `rara exec`. The Harbor adapter selects this
profile by default for Terminal-Bench 2.1 and records it in JSONL, AgentContext,
and ATIF metadata.

## Background

Apache Maka's Terminal-Bench 2.1 path freezes the executor, dataset, task set,
budget, verifier, runtime prompt, and tool profile. Its headless coding profile
uses a small exact tool surface and disables memory extraction. Maka's earlier
benchmark fixes also preserved provider reasoning fields across tool turns and
moved long-running command ownership into the task container.

RARA already preserves DeepSeek `reasoning_content` across tool turns and owns
background Bash and persistent PTY sessions inside the runtime. The remaining
composition gap was that Harbor used the full ambient application registry, so
plugins, memory, multi-agent policy, configured prompts, and unrelated tools
could change the benchmark arm without changing its name.

## Reference Revisions

- Apache Maka `9850304f16c5812c0beccd8c561da7470ef1d69e`, including its
  [Terminal-Bench 2.1 report](https://github.com/apache/maka/blob/9850304f16c5812c0beccd8c561da7470ef1d69e/docs/eval/terminal-bench-2.1-deepseek-v4-flash-four-arm.md).
- Harbor 0.20.0 at `459ff6ec99417589b7f679d14ddf3b3f0ae4f1dc`, matching the
  Maka comparison runtime.
- Harbor current main at `39b85872597ea710077d8c93095059bca3ca4ed2`, used to
  check forward compatibility with the 0.22 agent contract.

## Scope

- Add `RuntimeSessionProfile::{Default, HeadlessCodingV1}` as a public library
  contract.
- Project the headless profile in runtime bootstrap so CLI and embedded hosts
  use the same composition point.
- Freeze the profile prompt and exact shell, PTY, file, and search tool list.
- Disable ambient extensions, memory facilities, transcript persistence,
  automatic file-search context, and multi-agent orchestration.
- Add `rara exec --runtime-profile` and record the selection in JSONL.
- Honor the adapter's absolute `RARA_HOME` so every trial has an isolated
  config, OAuth, and workspace-state root. Headless commands no longer
  initialize interactive OAuth storage eagerly.
- Make the Harbor adapter select the profile, retain the metadata in ATIF, and
  target Terminal-Bench 2.1.
- Test the adapter against Harbor 0.20.0 and 0.22.0 in CI.

## Key Decisions

- The profile belongs to runtime assembly, not the TUI or Harbor adapter. The
  adapter selects a generic runtime capability but does not register tools or
  rewrite runtime state.
- Profile projection is an upper bound. A custom tool manager cannot silently
  widen a versioned profile.
- RARA keeps background Bash and PTY lifecycle tools in the profile instead of
  copying Maka's foreground-only Bash restriction. Terminal-Bench includes
  service and interactive-terminal tasks, and these are generic coding-agent
  capabilities rather than benchmark shortcuts.
- The default runtime remains unchanged. Any future prompt or tool change to
  the benchmark composition requires a new profile version.
- Official Harbor verifier reward remains authoritative. Adapter tests and a
  successful RARA exit prove integration only, not benchmark correctness.

## Validation

The implementation is covered by these focused checks:

```bash
cargo test --locked runtime_session::profile::tests
cargo test --locked -p rara-config rara_home
cargo test --locked oauth::tests::isolated_rara_home_keeps_oauth_storage_inside_the_override
cargo test --locked runtime_context::tests::headless_coding_profile_freezes_runtime_composition
cargo test --locked app_cli::tests
cargo test --locked exec_consumer::tests
cargo fmt --all -- --check
cargo check --locked --workspace --all-targets
cargo clippy --locked --workspace --all-targets -- -D warnings
```

The 2026-08-22 implementation checkpoint produced these focused results:

- the four profile-selection and composition tests passed;
- the four `RARA_HOME` tests and isolated OAuth-storage test passed;
- all 28 CLI tests and all eight exec-consumer tests passed;
- the Harbor adapter's 26 tests passed against both Harbor 0.20.0 and 0.22.0;
- workspace-wide Clippy completed with warnings denied;
- a mock-provider `rara exec` smoke emitted `headless-coding-v1` in JSONL and
  kept its only state file under the absolute trial-specific `RARA_HOME`.

The Rust test link step still reports the existing macOS compact-unwind
`__eh_frame` size warning. It does not appear in workspace Clippy and is not
caused by this runtime-profile change.

Focused Bazel validation did not reach loading or compilation because the
user-global Bazel configuration supplied an unsupported
`--experimental-disk-cache-gc-max-size=200G` startup option. The repository
Bazel configuration was not changed or bypassed.

The official Terminal-Bench 2.1 smoke remains separate because it requires a
running Docker daemon, a Linux RARA binary, provider credentials, and the
official verifier. Docker was unavailable at this checkpoint, so no benchmark
pass is claimed.

## Follow-Ups

- Run the recorded Terminal-Bench 2.1 smoke cohort and attach JSONL, ATIF, and
  official verifier artifacts before claiming a benchmark pass.
