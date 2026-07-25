# Runtime Prompt Slimming

## What Changed

The default runtime system prompt was trimmed to keep only durable cross-task invariants. Detailed
tool syntax, repeated validation procedures, verbose web-search routing, and duplicated workflow
rules were removed or collapsed into shorter contracts.

The prompt keeps the same section ordering and section keys so prompt assembly, prompt inspection,
and provider cache-prefix behavior stay stable. Skill guidance now describes the invocation contract
without embedding a step-by-step tool tutorial.

The prompt tests were moved into `crates/instructions/src/prompt/tests.rs` so
`crates/instructions/src/prompt.rs` stays below the repository source-file size limit while keeping
test coverage close to the prompt implementation.

## Why

Codex and Claude Code both keep their strongest general behavior in compact default prompts and push
tool-specific mechanics into tool descriptions, schemas, skills, workspace instructions, and
mode-specific addenda. RARA should follow that architecture: the always-on prompt should shape
reasoning and safety, while narrow procedural rules should live where the model needs them.

## Trade-offs

The prompt no longer repeats exact `apply_patch` grammar or long validation recipes. That makes the
default context smaller, but it relies on tool schemas and tool descriptions to provide call-time
syntax. Regression tests now assert the compact contract and guard against the old verbose snippets
returning to the default prompt.

## Remaining Work

- Continue moving narrow tool-selection and edit-syntax rules into tool descriptions when those
  descriptions are owned by RARA.
