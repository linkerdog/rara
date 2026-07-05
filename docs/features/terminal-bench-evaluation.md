# Terminal-Bench Evaluation Target

## Problem

RARA should be able to measure itself against Terminal-Bench as an external
terminal-agent benchmark. This gives the project a concrete product-quality
target beyond local unit tests and UI parity checks.

Terminal-Bench evaluates whether an agent can complete end-to-end tasks in a
terminal environment. The benchmark surface stresses repository inspection,
shell usage, file editing, dependency setup, long-running commands, recovery
from failures, and final task verification. These are core RARA capabilities.

## Scope

RARA should support a Terminal-Bench-compatible evaluation path that can:

- run under Harbor's Terminal-Bench tutorial flow:
  `harbor run -d terminal-bench/terminal-bench-2 -a <rara-agent>`;
- run RARA inside the benchmark task container;
- map benchmark task instructions into a single RARA session;
- expose the working directory and terminal environment without requiring TUI
  interaction;
- allow file edits, shell commands, and validation commands through the same
  tool/runtime contracts used by normal sessions;
- produce structured logs that can be inspected after each trial;
- make failures attributable to agent behavior, provider behavior, tool
  failures, sandbox limitations, or harness integration.

The initial target is compatibility and diagnosability, not leaderboard
optimization.

## Non-Goals

- Do not copy Terminal-Bench task content, oracle solutions, private tests, or
  benchmark data into RARA prompts, docs, fixtures, memories, or training data.
- Do not tune the default system prompt directly against known task answers.
- Do not make Terminal-Bench-specific shortcuts in core tools.
- Do not replace RARA's own focused unit and snapshot tests with benchmark
  runs.

## Architecture

The evaluation path should be an adapter around the existing runtime, not a
parallel agent implementation.

Recommended components:

- `rara exec` headless execution mode that reuses the normal agent loop
  without TUI chrome. Initial support includes prompt/stdin input,
  `--json` JSONL events, explicit cwd selection, run/task metadata, and
  `--output-last-message`.
- A Harbor installed-agent adapter that invokes `rara exec` for the task and
  converts RARA's structured output into ATIF-compatible trajectory artifacts.
  The first local integration ships as a dynamic Harbor import-path adapter:
  `PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2
  --agent rara_agent:RaraAgent --agent-kwarg
  binary_path=$PWD/target/release/rara`.
- `rara eval terminal-bench` may be added later as a convenience wrapper, but
  the first integration target is Harbor compatibility.
- Stable workspace setup contract:
  - cwd points at the benchmark task workspace;
  - all edits happen inside the task workspace unless the benchmark explicitly
    requires another path;
  - shell execution uses the same sandbox and approval policy as normal RARA
    sessions, with benchmark-specific defaults made explicit.
- Structured trajectory output:
  - user instruction;
  - assistant messages;
  - tool calls;
  - tool results;
  - file edit summaries;
  - command exit status and output tails;
  - final answer or failure reason.
- Per-run metadata:
  - RARA version / git revision;
  - provider and model;
  - sandbox mode;
  - token and tool-loop limits;
  - task id;
  - Harbor run id / trial id when provided by the harness;
  - start/end timestamps;
  - pass/fail result when provided by the harness.

## Contracts

### Adapter Contract

The benchmark adapter must present RARA as a terminal agent that can receive one
task instruction, operate inside the provided workspace, and stop with a final
answer when the task is complete.

For Harbor, the adapter should be an installed agent wrapper rather than a
benchmark-specific RARA runtime. The wrapper owns Harbor-specific plumbing:
reading the task input, invoking `rara exec`, and writing ATIF trajectory output.
RARA owns the generic headless agent execution and event stream.

The adapter must not require interactive TUI-only features. Any configuration
that is currently only exposed through `/model`, `/auth`, or overlays must also
have a headless path.

### Headless Execution Contract

`rara exec` is the stable automation surface:

- accept a prompt argument, stdin, or `-` for stdin-only prompts;
- support explicit cwd selection for task workspaces;
- support scriptable provider/model/API-key selection through existing config
  and CLI overrides;
- emit JSONL trajectory events when requested;
- optionally write the final assistant message to a file;
- fail fast when interactive approval, user input, or auth refresh is required
  in headless mode.

The JSONL event schema is RARA-owned and stable enough for the Harbor adapter
boundary. It includes thread start, turn start/completion/failure, assistant
message items, reasoning items, tool call/result/progress items, memory/todo
status items, model usage items, and final failure reasons. Later revisions can
add richer command exit metadata and file-change grouping without requiring a
benchmark-specific runtime.

### Tool Contract

The same file and shell tools used in ordinary sessions must be available in the
benchmark path. Evaluation should improve these generic tools instead of adding
benchmark-only behavior.

Important tool requirements:

- file edits are diff-shaped or source-aware enough to debug after failure;
- command output preserves enough stdout/stderr to diagnose build or test
  failures;
- long-running commands have explicit timeout and cancellation behavior;
- sandbox failures produce actionable diagnostics;
- tool-loop exhaustion reports whether the agent stopped without a final answer.

### Prompt Contract

The default prompt may describe general terminal-agent discipline:

- inspect before editing;
- prefer `rg` for search;
- use patch/file tools instead of shell redirection for edits;
- run focused verification;
- summarize unresolved failures.

The default prompt must not contain benchmark task answers, benchmark-specific
oracle behavior, or hidden test assumptions.

### Result Contract

Each trial should end with one of:

- `completed`: RARA reached a final answer and the harness can run validation;
- `agent_failed`: RARA reached a final answer but validation failed;
- `tool_failed`: a tool/runtime error prevented progress;
- `provider_failed`: the backend request failed or violated provider protocol;
- `budget_exceeded`: token, time, or tool-loop limits stopped the run;
- `adapter_failed`: the benchmark adapter failed before or around the agent run.

## Validation Matrix

- Run a small smoke subset locally through the adapter.
- Run Harbor's Terminal-Bench tutorial command with a RARA installed agent and
  record the exact `harbor run` invocation.
- For local dynamic-adapter smoke runs, build `target/release/rara` first and
  pass it through `--agent-kwarg binary_path=$PWD/target/release/rara`.
- Confirm the Harbor run receives an ATIF-compatible trajectory artifact.
- Confirm failures include enough trajectory data to reproduce the final
  decision.
- Confirm headless configuration can select provider/model/API key without TUI
  overlays.
- Confirm file edits and shell commands use the same runtime paths as normal
  sessions.
- Confirm benchmark data is not persisted into prompts, memories, specs, or
  training-oriented artifacts.
- Run the official benchmark harness once the adapter is stable and record the
  exact dataset version, RARA revision, provider, model, and run command.

## Open Risks

- Benchmark environments may require Docker or cloud container support that is
  unavailable in some local developer setups.
- Full benchmark runs are expensive and slow, so CI should start with a smoke
  subset instead of every task.
- Provider differences can hide RARA runtime issues; reports must always include
  provider/model metadata.
- TUI-only configuration or auth flows can block headless evaluation unless the
  config surface remains scriptable.
- Overfitting the prompt to public benchmark examples would make the benchmark
  result less meaningful.

## Source Journals

- `docs/journal/2026-07-05-rara-exec-headless.md`
- `docs/journal/2026-07-05-harbor-rara-agent-adapter.md`
