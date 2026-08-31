# Terminal-Bench Headless Lifecycle Validation

## Summary

The `terminal-bench/headless-terminal` rerun completed normally with no Harbor
errors but received verifier reward `0.0`. Six of seven checks passed. The
remaining failure was background-process behavior exercised through the
generated terminal interface.

## Evidence

- Harbor job: `jobs/2026-08-23__21-38-11`
- Trial: `headless-terminal__RepPUg9`
- Dataset task: `terminal-bench/headless-terminal`
- Harbor version: `0.17.1`
- RARA binary version: `0.0.22`
- RARA binary SHA-256:
  `01f963981b97aae31eedcde355aaed6f65d9a60bc08611a45f083940cda5e1ba`
- Repository revision:
  `806839690f4b0a38d76d5da074e94932dc4cbd7f`
- Provider/model: DeepSeek / `deepseek-v4-pro`
- RARA exit status: `0`
- Harbor trial errors: `0`
- Official verifier reward: `0.0`
- Usage: `1,331,138` input tokens and `27,239` output tokens

The trajectory shows focused validation of several foreground, interactive,
signal, and stateful behaviors, but no direct background-process validation
through the completed artifact. The final response therefore claimed broader
completion than its recorded evidence supported.

The run configuration also supplied `reasoning_effort=high`, but the Harbor
adapter did not recognize or forward that kwarg. The underlying RARA config
supports reasoning effort, so silently accepting the kwarg made the benchmark
configuration misleading.

## Reference Patterns And Adaptation Plan

- Codex app-server revision
  [`6ca61345ceb09d76edc3db8c4eb55df18a10888a`](https://github.com/openai/codex/blob/6ca61345ceb09d76edc3db8c4eb55df18a10888a/codex-rs/app-server/tests/suite/v2/review.rs)
  keeps inline review on the parent thread but gives detached review a distinct
  `review_thread_id`. The response exposes that child identity instead of
  replacing the parent identity.
- Claude Code documents non-fork
  [subagents](https://code.claude.com/docs/en/sub-agents) as fresh isolated
  contexts whose result returns as a summary while the subagent transcript and
  lifecycle remain separate.

RARA adapts those boundaries rather than copying either review surface. The
verification pass starts a fresh RARA session in the same benchmark workspace,
receives only the original task and an untrusted implementation summary, and
keeps its own task ID and artifacts. The combined Harbor trajectory retains the
implementation session as its envelope identity and records ordered per-pass
session metadata so downstream consumers can inspect both runs without
collapsing them.

## Implementation

- Add process-local `--reasoning-effort` and `--thinking` CLI overrides.
- Forward both controls from the Harbor adapter. For DeepSeek, an explicit
  reasoning effort enables thinking unless the caller explicitly disables it.
- Build the Linux benchmark upload for `x86_64-unknown-linux-musl`. Vendored
  OpenSSL is enabled for Linux musl targets so the binary does not depend on
  target-container OpenSSL or glibc availability.
- Require a short validation checklist derived from requested behaviors and
  constraints before editing.
- Require wrappers, emulators, protocol clients, and process controllers to
  exercise applicable lifecycle modes through their public interface.
- Generalize background-service validation so it applies during artifact
  validation, not only when an external verifier connects after agent exit.
- Add a final evidence-to-requirements reconciliation and bounded guidance to
  stop dependency investigation after a direct behavior check resolves it.

These rules remain task-agnostic. They do not include benchmark answers,
private verifier logic, or fixed task-specific values.

## Validation

- `PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=tools/harbor
  /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest
  tools.harbor.test_rara_agent tools.harbor.test_rara_agent_prompts
  tools.harbor.test_rara_agent_trajectory`: 38 passed.
- `cargo test app_cli::tests -- --nocapture`: 29 passed.
- `cargo clippy --all-targets -- -D warnings`: passed.
- `cargo fmt --check`: passed.
- `git diff --check`: passed.
- `cargo build --locked --release --target x86_64-unknown-linux-musl --bin rara`:
  passed.
- `file target/x86_64-unknown-linux-musl/release/rara`: reports an x86-64
  static PIE executable.
- `ldd target/x86_64-unknown-linux-musl/release/rara`: reports statically
  linked.
- `readelf -lW` contains no `INTERP` program header and `readelf -dW` contains
  no `NEEDED` dynamic dependency.
- The musl binary reports `rara 0.0.22`, is `92,452,808` bytes, and has
  SHA-256
  `9ec04a596f57bfc0bab3865213b7d0f7a157fa7b6075a044b54c9b7e5883bc20`.

## 2026-08-24 Musl Smoke Rerun

Harbor job `jobs/2026-08-24__00-28-08` confirmed that the musl binary installs
and executes inside the task container. The trial completed with RARA exit
status `0`, no harness errors, and official verifier reward `0.0`: six checks
passed and the background-command lifecycle check failed again.

This run did not exercise the intended provider configuration. Its recorded
agent kwargs contain only the musl `binary_path`; the generated RARA command
selected the inferred DeepSeek provider with the default `deepseek-chat` model
and did not include model, reasoning-effort, or thinking overrides. The
trajectory contains no background-process launch or separate-client validation,
although the agent's final message claimed complete validation.

Compared with the prior run, usage dropped from `1,331,138` input and `27,239`
output tokens to `222,792` input and `7,528` output tokens, and total runtime
dropped to 2 minutes 45 seconds. This is useful efficiency evidence but does
not close the behavior regression.

The next rerun must preserve the same musl binary while explicitly passing the
planned provider, model, reasoning-effort, and thinking kwargs. No additional
task-specific prompt guidance is added from the verifier failure.

## 2026-08-24 High-Thinking Rerun

Harbor job `jobs/2026-08-24__09-43-31` was the first valid rerun of the complete
configuration. Its job artifact records the musl binary, DeepSeek provider,
`deepseek-v4-pro`, high reasoning effort, and thinking enabled; JSONL model
events confirm `deepseek-v4-pro` handled the turn.

The trial completed with RARA exit status `0`, no harness errors, and official
verifier reward `0.0`. Six checks passed and the background-command lifecycle
check failed again. Usage was `211,466` input tokens and `10,419` output tokens.

The trajectory shows that the agent implemented and checked foreground command
execution, interactive input, signal handling, startup files, and compilation,
but never launched or externally observed long-running or detached work through
the completed interface. Its final message nevertheless claimed the interface
was complete. This falsifies the assumption that a completion rule placed only
before a long raw task remains sufficiently visible at the final-answer
boundary.

The adapter now repeats a concise, task-agnostic completion gate after the raw
task text. It requires direct public-interface evidence for applicable
interaction and lifecycle modes and requires failed or unrun checks to remain
explicit. The gate does not include task-specific commands, ports, expected
responses, or private verifier details.

## 2026-08-24 Post-Task Gate Rerun

Harbor job `jobs/2026-08-24__11-50-44` loaded the post-task completion gate and
the full musl, `deepseek-v4-pro`, high-reasoning, thinking-enabled configuration.
The trial completed with RARA exit status `0`, no harness errors, and official
verifier reward `0.0`. Six checks passed and the background-command lifecycle
check failed again. Usage was `322,746` input tokens and `17,445` output tokens.

The recorded instruction confirms that the completion gate appeared after the
raw task. The trajectory nevertheless contains no background service launch,
readiness polling, or separate-client request. The agent built two foreground
validation scripts, checked import, interactive input, startup files, Ctrl-C,
session state, and cleanup, then claimed completion. This falsifies prompt
placement as a sufficient completion boundary.

The adapter now uses a bounded independent verification-and-repair pass after a
successful implementation pass. The second RARA session re-reads the original
task, inspects the existing artifact without trusting the first summary, maps
the public interface to lifecycle risks, runs missing behavior checks, and may
repair failures. It uses separate instruction, task ID, status, last-message,
and JSONL artifacts. The combined ATIF trajectory and token metrics include both
passes. `verification_pass=false` remains the explicit cost/latency opt-out.

This remains task-agnostic: the review pass is forbidden from depending on
benchmark verifier code and contains no fixed commands, ports, expected task
outputs, or private oracle content.

## 2026-08-24 Independent Verification Rerun

Harbor job `jobs/2026-08-24__12-30-48` confirmed that the independent pass ran
with separate instruction, JSONL, status, and final-message artifacts. Both RARA
sessions exited `0` with no Harbor exception, but the official verifier reward
remained `0.0`: six checks passed and background-command behavior again returned
HTTP `503`. Combined usage was `988,819` input tokens and `37,161` output tokens.

The verification session used `517,932` input tokens, `17,982` output tokens,
25 model requests, and 38 shell calls. It repeated foreground execution,
interactive input, Ctrl-C, startup-file, cwd, and cleanup checks already covered
by the implementation pass. It then investigated and changed large-output drain
behavior, which was outside the required lifecycle matrix. Its final behavior
matrix still omitted background or detached child execution. The only late
reference to background work was a cleanup thought about leaving no background
tasks, not a public-interface behavior check.

The fresh reviewer lacked the implementation pass's evidence summary, so it had
no concrete delta and reconstructed the same familiar checks from scratch. The
adapter now injects that summary as an untrusted coverage map. The reviewer must
start with applicable artifact-class risks missing from the report, avoid
repeating evidenced checks unless a repair can affect them, and defer optional
performance, scale, or robustness exploration until the matrix is complete.
This changes reviewer input and work ordering without adding a third pass or
embedding verifier commands, ports, task answers, or private oracle content.

## 2026-08-31 PR Hardening

Review of the combined trajectory path found that treating two independent
`thread.started` events as one stream allowed the verification session to
replace the implementation session's top-level ATIF identity. Global
final-message deduplication could also suppress a valid verification response
when both passes returned the same text.

The adapter now:

- preserves the implementation session as the combined trajectory identity;
- records ordered `implementation` and `verification` metadata under
  `final_metrics.extra.rara_sessions`;
- resets turn-local model, reasoning, tool-call, and message-deduplication state
  at pass boundaries;
- namespaces tool-call IDs by pass and infers the verification boundary when
  reconstructing ATIF directly from the combined raw JSONL stream;
- records verification lifecycle as `disabled`, `not_started`, `failed`, or
  `completed` and publishes the verification JSONL path only after invocation.
- keeps the adapter below the source-size limit by extracting pure prompt and
  option helpers into `rara_agent_prompts.py` with focused tests.

Focused regression coverage exercises the complete two-session event shape,
equal final messages, aggregate token usage, explicit opt-out, implementation
failure before verification, and verification artifact visibility after start.
The validation commands for this checkpoint are:

```bash
PYTHONDONTWRITEBYTECODE=1 PYTHONPATH=tools/harbor \
  /home/hawkingrei/.local/share/uv/tools/harbor/bin/python -m unittest \
  tools.harbor.test_rara_agent tools.harbor.test_rara_agent_prompts \
  tools.harbor.test_rara_agent_trajectory
cargo test app_cli::tests -- --nocapture
cargo fmt --all --check
git diff --check
```

## Follow-Ups

- Rerun `terminal-bench/headless-terminal` with the evidence-delta verification
  pass, fresh musl release binary, and active DeepSeek controls.
- Inspect both `rara-exec.jsonl` and `rara-verification.jsonl` to confirm the
  second pass directly exercises the missed lifecycle behavior.
- Treat only an official verifier reward of `1.0` as closure, then expand to a
  multi-task cohort.
