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

### Official Terminal-Bench 2.1 smoke

Docker became available after the initial implementation checkpoint. A
content-addressed `terminal-bench/regex-log` run completed on 2026-08-23 with
these frozen inputs:

- RARA commit `94a5ddf1c8dc5c8d789b6e0a15cab1fdbd6410d3` and version
  `0.0.21`;
- Harbor `0.20.0` at `459ff6ec99417589b7f679d14ddf3b3f0ae4f1dc`;
- Terminal-Bench 2.1 dataset ref
  `sha256:7d7bdc1cbedad549fc1140404bd4dc45e5fd0ea7c4186773687d177ad3a0699a`;
- `terminal-bench/regex-log` package ref
  `sha256:802c16cfd132e6c457529cb864be5a757c1b23b6cadc57f2d01983cb0110292a`;
- the `alexgshaw/regex-log:20251031` `linux/amd64` task image at
  `sha256:90101b2e815323a8da20528a1439bebc407eb9761c9c68a3d557730856c878e9`;
- an `x86-64` Linux RARA binary with SHA-256
  `12fda6286efd4a0460eb95b52d0be260be57172f98f3cae0ac21ca8c716a2e01`;
- the official DeepSeek API with provider `deepseek`, model
  `deepseek-v4-pro`, and runtime profile `headless-coding-v1`.

The registry resolved the dataset and downloaded the task package before a
later metadata request became unavailable. The scored run therefore used
Harbor's local `--path` mode against that exact content-addressed task package;
the task container, task instructions, and official verifier were unchanged.
The provider credential remained in the process environment and was selected
through `api_key_env=DEEPSEEK_API_KEY`; it was not written into the command or
artifacts.

The successful Harbor invocation was:

```bash
PYTHONPATH=$PWD/tools/harbor harbor run \
  --path "$TERMINAL_BENCH_REGEX_LOG_TASK_DIR" \
  --n-concurrent 1 \
  --agent rara_agent:RaraAgent \
  --agent-kwarg binary_path="$RARA_LINUX_AMD64_BIN" \
  --agent-kwarg provider=deepseek \
  --agent-kwarg model=deepseek-v4-pro \
  --agent-kwarg api_key_env=DEEPSEEK_API_KEY \
  --jobs-dir "$HARBOR_JOBS_DIR" \
  --job-name rara-tbench21-regex-log-94a5ddf-20260823-local \
  --yes
```

Harbor job `6c3f7fe6-ed08-4514-8541-50b9d1b1ad6e` completed one trial with
zero exceptions in 8 minutes 2 seconds. The RARA process status was `0`, and
the official verifier reward was `1.0`. The ATIF-v1.7 trajectory recorded RARA
`0.0.21`, `deepseek-v4-pro`, `headless-coding-v1`, 249 steps, 226,386 input
tokens, and 25,722 output tokens.

The local evidence was retained outside the repository. Its primary artifact
hashes are:

- trial `result.json`:
  `69cc798967665cda07fe30098202c7d8d6f54cb033a0cf071631b6f8c9c7d346`;
- verifier `reward.txt`:
  `4355a46b19d348dc2f57c046f8ef63d4538ebb936000f3c9ee954a27460dd865`;
- raw `rara-exec.jsonl`:
  `16a4700cd52aaa5d516502e83093f5158c25c3e2b5a6839c670237ee67ef7ede`;
- ATIF `trajectory.json`:
  `0aec4b6abf9a2a9e1fce463f8e78e8560d7977322ee8338adb3306e2d2018468`.

The selected artifacts and a content manifest were bundled locally as
`rara-tbench21-regex-log-94a5ddf-20260823-evidence.tar.gz`, with SHA-256
`d39dcabc04425a16e8b16be6c86cdf165a1e18dee8a48f1b9281022a189cad98`.
No task prompt, trajectory, model payload, or verifier implementation was
committed to the repository; the trajectory exists only in that local evidence
bundle.

The PR's Bazel CI subsequently passed. An earlier local Bazel attempt did not
reach loading or compilation because the user-global Bazel configuration
supplied an unsupported `--experimental-disk-cache-gc-max-size=200G` startup
option; the repository Bazel configuration was not changed or bypassed.

## Follow-Ups

- Expand the single-task smoke into a multi-task cohort and repeat selected
  tasks before reporting a suite-level score. The recorded reward proves the
  end-to-end RARA path can pass an official Terminal-Bench 2.1 verifier, but it
  is not a full-suite result or a variance estimate.
