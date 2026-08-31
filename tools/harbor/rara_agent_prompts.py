"""Task-agnostic prompt builders and argument parsing for the Harbor adapter."""

from __future__ import annotations

from typing import Any


def build_benchmark_instruction(instruction: str, cwd: str) -> str:
    """Wrap Harbor task text with generic non-interactive benchmark guidance."""
    return f"""You are running inside a non-interactive Terminal-Bench task container.

Work only in the benchmark workspace: {cwd}.
Read the task carefully and create every file path that the task asks for exactly as specified.
If the task names an absolute output path under the workspace, write that artifact before you finish.
Prefer dedicated file tools over shell commands for file reads and edits. \
Use read_file to inspect files, and use apply_patch or write_file for file \
modifications. Do not use shell redirection, \
heredocs, sed, awk, perl, or ad-hoc scripts to edit files when a direct edit \
tool can do the job.
Use shell commands for process execution and focused validation commands.
Use bash with run_in_background for long-running non-interactive processes, then poll the background task status. Use PTY tools only for commands that require terminal input or terminal control.
Before editing, turn every requested behavior and explicit constraint into a short validation checklist. For wrappers, emulators, protocol clients, or process controllers, exercise every requested interaction and lifecycle mode through the artifact's public interface, including background-process behavior when applicable.
For every background process, daemon, or network service involved in the requested behavior, exercise its launch through the surface under test, poll readiness, and verify it from a separate client or process before finishing. Do not infer success from launch output, a PID, or a process listing alone. Clean up temporary processes unless the task requires them to remain running.
Treat task constraints as validation requirements. If the task says only certain edits are allowed, files must not be edited, output must match an exact format, or substitutions must come from an allowed list, verify those constraints directly before finishing.
When a task requires a command to be available in PATH, verify it in a fresh non-interactive process without a command-local PATH export. Updating shell startup files alone does not prove that the verifier can resolve the command.
Prefer the smallest implementation that satisfies the stated interface. Once a direct behavior check answers a question, stop investigating dependency internals and move to uncovered checklist items. Before finishing, compare the implementation and validation evidence against the original task and report any unverified requirement instead of claiming completion.
When running shell commands, request escalated sandbox permissions; Harbor already isolates this task inside its container.
Do not finish with only an explanation. Finish only after the requested artifacts exist, or report the exact blocker if you cannot create them.

Task instructions:
{instruction.strip()}

Completion gate:
Before your final answer, re-read the task and the generic guidance above. Do not claim an interface is complete unless every applicable interaction and lifecycle mode has a direct behavior check through the artifact's public interface. Long-running or detached work must be observed from a separate client or process rather than inferred from launch output, a PID, or a process listing. If a required check fails or was not run, keep working or report it as unverified.
"""


def build_verification_instruction(
    instruction: str,
    cwd: str,
    *,
    implementation_summary: str | None,
) -> str:
    """Build an evidence-delta review-and-repair pass for benchmark work."""
    reported_evidence = implementation_summary or "No implementation summary was recorded."
    return f"""You are the independent final verification and repair pass for a completed coding-agent task.

Work only in the benchmark workspace: {cwd}.
Treat the existing implementation and its prior summary as untrusted evidence. Do not search for or depend on benchmark verifier code. Derive the contract from the original task, the public interface, and the artifact itself.

Original task:
{instruction.strip()}

Reported implementation and validation evidence:
{reported_evidence.strip()}

Evidence-delta review protocol:
1. Build a compact behavior matrix from the original task and artifact class.
2. Use the reported evidence only to identify checks already attempted; do not trust a pass claim without direct artifact evidence when inspecting or repairing it.
3. Do not repeat already evidenced checks unless a repair can affect them. Start with applicable matrix items missing from the reported evidence.
4. Process controllers and terminal-like interfaces require foreground commands, interactive input, control or signal handling, startup environment, state across calls, and background or detached child behavior. Services and long-running processes require readiness polling and an observation from a separate client or process.
5. Do not explore optional performance, scale, or robustness improvements until every applicable matrix item has direct evidence.

Reproduce uncovered failures through the public interface, make the smallest robust repair, and rerun only affected checks. Preserve correct existing work and task constraints. Remove temporary validation artifacts and processes before finishing.

Completion condition:
Every applicable matrix item must have direct evidence. An item absent from the reported evidence remains unverified until you check it. If a required behavior cannot be verified, report the exact blocker instead of claiming completion.
"""


def last_completed_message(events: list[dict[str, Any]]) -> str | None:
    """Return the most recent completed-turn message from an event stream."""
    for event in reversed(events):
        if event.get("type") != "turn.completed":
            continue
        message = event.get("final_message")
        if isinstance(message, str) and message.strip():
            return message
    return None


def parse_optional_bool(value: bool | str | None, *, name: str) -> bool | None:
    """Parse Harbor string kwargs without collapsing an omitted value to false."""
    if value is None or isinstance(value, bool):
        return value
    normalized = value.strip().lower()
    if normalized in {"1", "true", "yes", "on"}:
        return True
    if normalized in {"0", "false", "no", "off"}:
        return False
    raise ValueError(f"{name} must be a boolean value, got {value!r}")
