# Support ACP Integration

## Problem

RARA has an ACP surface and a `support-acp` skill, but the integration contract
should not live only in the skill text. External IDEs, protocol clients, and
future appserver adapters need a stable feature spec that explains what the
skill is allowed to teach and which runtime-control contracts it depends on.

Without that spec, the skill can drift into product-specific instructions while
the runtime evolves independently.

## Scope

This spec covers the support material for third-party applications that connect
to RARA through ACP or ACP-shaped adapters.

In scope:

- the purpose and boundary of the `support-acp` skill;
- ACP startup and session lifecycle guidance;
- semantic input intents for prompts, queued follow-ups, pending answers,
  approvals, cancellation, and interruption;
- output subscription and structured event expectations;
- safe registration of prompt, skill, memory, MCP, and future hook sources;
- control-plane provenance and trust expectations;
- update rules that keep the skill aligned with runtime behavior.

Out of scope:

- replacing `docs/features/runtime-control-plane.md`;
- defining the upstream ACP protocol;
- promising that every future ACP client can bypass RARA's local approval,
  sandbox, memory, or prompt-source policies;
- documenting internal implementation details that are not visible to clients.

## Architecture

`support-acp` is a consumer-facing integration skill layered on top of RARA's
runtime control plane.

The layering is:

1. `runtime-control-plane.md` defines RARA-owned request, event, provenance,
   approval, source, and subscription contracts.
2. ACP, Wire, TUI, CLI, and future appserver adapters translate their protocol
   messages into those runtime-control contracts.
3. `support-acp` explains the ACP-facing subset to external client authors.

The skill is documentation, not an alternate runtime path. It must not describe
direct prompt concatenation, direct LanceDB access, TUI key simulation, or
adapter-specific state mutations as supported integration methods.

## Contracts

### Skill Placement

The canonical skill entrypoint is:

```text
.agents/skills/support-acp/SKILL.md
```

The skill should stay in the repository skill tree so users and protocol client
authors get the project-specific integration contract when they work inside
RARA.

### Semantic Input

ACP clients should send semantic runtime intents instead of terminal events.

Required intent mapping:

| Client action | Runtime intent |
| --- | --- |
| submit a prompt | `InputControlRequest::SubmitUserPrompt` |
| submit while a turn is busy | `InputControlRequest::SubmitFollowUp` |
| answer a request-input prompt | `InputControlRequest::AnswerPendingInput` |
| approve, continue, or reject a plan | `InputControlRequest::AnswerPlanApproval` |
| approve or deny a shell command | `InputControlRequest::AnswerShellApproval` |
| cancel the current turn | `SessionControlRequest::CancelCurrentTurn` |
| request preemption | `SessionControlRequest::InterruptCurrentTurn` |

Clients must not send raw keys such as `Esc` or rely on TUI overlay behavior.
Queued follow-ups preserve input order. Cancellation and interruption are
separate control requests and must not be implied by a follow-up.

### Output Events

ACP-facing output should be derived from structured runtime events, not parsed
from TUI text.

The support skill should document at least these event families:

- assistant text;
- reasoning or thinking;
- tool lifecycle;
- tool stdout, stderr, and system streams;
- approvals;
- request-input prompts;
- plan and todo updates;
- context and memory observability;
- warnings, errors, cancellation, and completion.

Plain text streaming is a presentation layer. The source of truth is the
runtime-control event stream.

### Source Registration

External clients may contribute prompt-affecting material only through
structured source objects.

Supported source families:

- prompt sources;
- skill sources;
- memory requests and memory records;
- MCP resources, prompts, and tools;
- hook declarations after the hook policy is enabled.

Each source must carry provenance, source id, scope, lifetime, trust, authorship,
and budget metadata where applicable. Sources that can affect the stable prompt
prefix must be deterministic and ordered by runtime policy, not by client
arrival race.

### Trust And Approval

ACP clients are untrusted unless the runtime explicitly marks a source or
operation as trusted. Trust does not bypass:

- sandbox policy;
- shell approval policy;
- memory mutation validation;
- prompt-source provenance checks;
- hook execution policy.

If a client requests a privileged action, RARA should route it through the same
approval and permission system used by the local TUI.

## Validation Matrix

- The support skill links back to runtime-control, prompt-source, memory, MCP,
  and context specs rather than redefining incompatible contracts.
- ACP client examples use semantic runtime intents, not TUI key events.
- Output examples are event-based and do not require scraping rendered text.
- Source registration examples include provenance and lifetime metadata.
- Changes to runtime-control request or event shapes update the support skill
  in the same PR.

## Open Risks

- ACP and Wire may expose different transport semantics. The skill should
  describe RARA runtime intents first and only then show protocol-specific
  mappings.
- Some source families are scaffolds rather than complete runtime execution
  paths. The skill should mark these clearly instead of implying support is
  broader than the implementation.
- Future appserver clients may need UI-control requests for overlays and
  pickers. Those should remain separate from agent input intents.

## Source Journals

- [Runtime Control Plane](runtime-control-plane.md)
- [MCP Runtime](mcp-runtime.md)
- [Prompt Runtime](prompt-runtime.md)
- [Memory Records](memory-records.md)
