"""Harbor adapter for running RARA through ``rara exec``.

Load dynamically with:

    PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2-1 \
      --agent rara_agent:RaraAgent \
      --agent-kwarg binary_path=$PWD/target/release/rara
"""

from __future__ import annotations

import json
import os
import shlex
from enum import Enum
from pathlib import Path, PurePosixPath
from typing import Any

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.trajectories import (
    Agent,
    FinalMetrics,
    Metrics,
    Observation,
    ObservationResult,
    Step,
    ToolCall,
    Trajectory,
)
from harbor.models.trial.paths import EnvironmentPaths
from harbor.utils.trajectory_utils import format_trajectory_json
from rara_agent_prompts import (
    build_benchmark_instruction,
    build_verification_instruction,
    last_completed_message,
    parse_optional_bool,
)


DEFAULT_REMOTE_BINARY = PurePosixPath("/installed-agent/rara")
DEFAULT_RARA_HOME = PurePosixPath("/logs/agent/rara-home")
DEFAULT_JSONL_PATH = EnvironmentPaths.agent_dir / "rara-exec.jsonl"
DEFAULT_EXIT_STATUS_PATH = EnvironmentPaths.agent_dir / "rara-exec.status"
DEFAULT_INSTRUCTION_PATH = EnvironmentPaths.agent_dir / "instruction.txt"
DEFAULT_IMPLEMENTATION_LAST_MESSAGE_PATH = (
    EnvironmentPaths.agent_dir / "implementation-last-message.txt"
)
DEFAULT_VERIFICATION_JSONL_PATH = EnvironmentPaths.agent_dir / "rara-verification.jsonl"
DEFAULT_VERIFICATION_EXIT_STATUS_PATH = (
    EnvironmentPaths.agent_dir / "rara-verification.status"
)
DEFAULT_VERIFICATION_INSTRUCTION_PATH = (
    EnvironmentPaths.agent_dir / "verification-instruction.txt"
)
DEFAULT_LAST_MESSAGE_PATH = EnvironmentPaths.agent_dir / "last-message.txt"
DEFAULT_TRAJECTORY_PATH = EnvironmentPaths.agent_dir / "trajectory.json"
DEFAULT_BENCHMARK_CWD = "/app"
DEFAULT_RUNTIME_PROFILE = "headless-coding-v1"
CA_CERTIFICATE_BUNDLE = PurePosixPath("/etc/ssl/certs/ca-certificates.crt")
PROVIDER_API_KEY_ENVS = {
    "deepseek": ("DEEPSEEK_API_KEY",),
    "gemini": ("GEMINI_API_KEY",),
    "kimi": ("KIMI_API_KEY", "MOONSHOT_API_KEY"),
    "openrouter": ("OPENROUTER_API_KEY",),
    "codex": ("CODEX_API_KEY", "OPENAI_API_KEY"),
    "openai-compatible": ("RARA_API_KEY", "OPENAI_API_KEY"),
}
PROVIDER_INFERENCE_ORDER = ("deepseek", "kimi", "gemini", "openrouter", "codex")


class VerificationStatus(str, Enum):
    """Track whether the optional verification process actually ran."""

    DISABLED = "disabled"
    NOT_STARTED = "not_started"
    FAILED = "failed"
    COMPLETED = "completed"


class RaraAgent(BaseInstalledAgent):
    """Run RARA as a Harbor installed agent using the headless exec surface."""

    SUPPORTS_ATIF = True
    SUPPORTS_WINDOWS = False

    def __init__(
        self,
        *args: Any,
        binary_path: str | None = None,
        remote_binary: str | None = None,
        cwd: str = DEFAULT_BENCHMARK_CWD,
        rara_home: str | None = None,
        provider: str | None = None,
        model: str | None = None,
        reasoning_effort: str | None = None,
        thinking: bool | str | None = None,
        base_url: str | None = None,
        api_key_env: str | None = None,
        runtime_profile: str = DEFAULT_RUNTIME_PROFILE,
        verification_pass: bool | str = True,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        self.binary_path = Path(
            binary_path or os.environ.get("RARA_HARBOR_BINARY", "target/release/rara")
        ).expanduser().resolve()
        self.remote_binary = PurePosixPath(remote_binary or DEFAULT_REMOTE_BINARY)
        self.cwd = cwd
        self.rara_home = PurePosixPath(rara_home or DEFAULT_RARA_HOME)
        self.provider = provider
        self.model = model
        self.reasoning_effort = reasoning_effort
        self.thinking = parse_optional_bool(thinking, name="thinking")
        self.base_url = base_url
        self.api_key_env = api_key_env
        self.runtime_profile = runtime_profile or DEFAULT_RUNTIME_PROFILE
        parsed_verification_pass = parse_optional_bool(
            verification_pass, name="verification_pass"
        )
        self.verification_pass = (
            True if parsed_verification_pass is None else parsed_verification_pass
        )

    @staticmethod
    def name() -> str:
        return "rara"

    def version(self) -> str | None:
        return self._version

    def get_version_command(self) -> str | None:
        return f"{shlex.quote(self.remote_binary.as_posix())} --version"

    async def install(self, environment: BaseEnvironment) -> None:
        if not self.binary_path.is_file():
            raise FileNotFoundError(
                "RARA binary not found. Build it first or pass "
                f"--agent-kwarg binary_path=/path/to/rara. Looked at: {self.binary_path}"
            )

        await environment.upload_file(
            self.binary_path,
            self.remote_binary.as_posix(),
        )
        await self.exec_as_root(
            environment,
            command=f"chmod 0755 {shlex.quote(self.remote_binary.as_posix())}",
        )
        await self.exec_as_root(
            environment,
            command=self._ca_certificate_install_command(),
            timeout_sec=180,
        )
        validation = await environment.exec(
            command=f"{shlex.quote(self.remote_binary.as_posix())} --version",
            user="root",
            timeout_sec=30,
        )
        if validation.return_code != 0:
            output = validation.stderr or validation.stdout or "no output"
            raise RuntimeError(
                "Uploaded RARA binary cannot run inside the Harbor environment. "
                "For Docker-backed Terminal-Bench runs, pass a Linux binary via "
                "--agent-kwarg binary_path=/path/to/linux/rara. "
                f"Host binary: {self.binary_path}. "
                f"Remote error: {output.strip()}"
            )

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        instruction_path = self.logs_dir / "instruction.txt"
        instruction_path.parent.mkdir(parents=True, exist_ok=True)
        cwd = self.effective_cwd()
        benchmark_instruction = build_benchmark_instruction(instruction, cwd)
        instruction_path.write_text(benchmark_instruction, encoding="utf-8")
        await environment.upload_file(
            instruction_path,
            DEFAULT_INSTRUCTION_PATH.as_posix(),
        )

        env = {
            **self._provider_env(),
            **self.extra_env,
            "RARA_HOME": self.rara_home.as_posix(),
        }
        verification_status = (
            VerificationStatus.NOT_STARTED
            if self.verification_pass
            else VerificationStatus.DISABLED
        )
        implementation_last_message_path = (
            DEFAULT_IMPLEMENTATION_LAST_MESSAGE_PATH
            if self.verification_pass
            else DEFAULT_LAST_MESSAGE_PATH
        )
        command = self._build_exec_command(
            last_message_path=implementation_last_message_path
        )
        result = await environment.exec(command=command, cwd=cwd, env=env)

        events = parse_rara_jsonl(result.stdout or "")
        if self._completed_with_mock_backend(events):
            self._record_run(
                context,
                instruction=benchmark_instruction,
                events=events,
                verification_status=verification_status,
            )
            raise RuntimeError(
                "RARA completed with the mock backend. Pass provider/model credentials "
                "to the Harbor adapter, for example "
                "--agent-kwarg provider=gemini --agent-kwarg model=gemini-2.5-flash, "
                "or expose a supported provider API key in the Harbor process environment."
            )
        if result.return_code != 0:
            self._record_run(
                context,
                instruction=benchmark_instruction,
                events=events,
                verification_status=verification_status,
            )
            raise self._classify_exec_error(command, result)

        if self.verification_pass:
            implementation_summary = last_completed_message(events)
            verification_instruction = build_verification_instruction(
                instruction,
                cwd,
                implementation_summary=implementation_summary,
            )
            verification_instruction_path = self.logs_dir / "verification-instruction.txt"
            verification_instruction_path.write_text(
                verification_instruction, encoding="utf-8"
            )
            await environment.upload_file(
                verification_instruction_path,
                DEFAULT_VERIFICATION_INSTRUCTION_PATH.as_posix(),
            )
            verification_command = self._build_exec_command(
                instruction_path=DEFAULT_VERIFICATION_INSTRUCTION_PATH,
                jsonl_path=DEFAULT_VERIFICATION_JSONL_PATH,
                status_path=DEFAULT_VERIFICATION_EXIT_STATUS_PATH,
                last_message_path=DEFAULT_LAST_MESSAGE_PATH,
                combined_jsonl_path=DEFAULT_JSONL_PATH,
                task_id_suffix="-verification",
            )
            verification_status = VerificationStatus.FAILED
            try:
                verification_result = await environment.exec(
                    command=verification_command, cwd=cwd, env=env
                )
            except Exception:
                self._record_run(
                    context,
                    instruction=benchmark_instruction,
                    events=events,
                    verification_status=verification_status,
                )
                raise
            verification_events = parse_rara_jsonl(verification_result.stdout or "")
            events.append(
                {
                    "type": "adapter.user_message",
                    "phase": "verification",
                    "message": verification_instruction,
                }
            )
            events.extend(verification_events)
            if self._completed_with_mock_backend(verification_events):
                self._record_run(
                    context,
                    instruction=benchmark_instruction,
                    events=events,
                    verification_status=verification_status,
                )
                raise RuntimeError(
                    "RARA verification completed with the mock backend. Pass "
                    "provider/model credentials to the Harbor adapter."
                )
            if verification_result.return_code != 0:
                self._record_run(
                    context,
                    instruction=benchmark_instruction,
                    events=events,
                    verification_status=verification_status,
                )
                raise self._classify_exec_error(
                    verification_command, verification_result
                )
            verification_status = VerificationStatus.COMPLETED

        self._record_run(
            context,
            instruction=benchmark_instruction,
            events=events,
            verification_status=verification_status,
        )

    def _build_exec_command(
        self,
        *,
        instruction_path: PurePosixPath = DEFAULT_INSTRUCTION_PATH,
        jsonl_path: PurePosixPath = DEFAULT_JSONL_PATH,
        status_path: PurePosixPath = DEFAULT_EXIT_STATUS_PATH,
        last_message_path: PurePosixPath = DEFAULT_LAST_MESSAGE_PATH,
        combined_jsonl_path: PurePosixPath | None = None,
        task_id_suffix: str = "",
    ) -> str:
        binary = shlex.quote(self.remote_binary.as_posix())
        cwd = shlex.quote(self.effective_cwd())
        quoted_jsonl_path = shlex.quote(jsonl_path.as_posix())
        quoted_status_path = shlex.quote(status_path.as_posix())
        quoted_instruction_path = shlex.quote(instruction_path.as_posix())
        quoted_last_message_path = shlex.quote(last_message_path.as_posix())
        run_id = shlex.quote(self.context_id.hex if self.context_id else "harbor")
        task_id = shlex.quote(f"{self.session_id or 'harbor-task'}{task_id_suffix}")
        global_flags = self._build_rara_global_flags()
        binary_invocation = f"{binary} {global_flags}".rstrip()
        jsonl_sink = f"tee {quoted_jsonl_path}"
        if combined_jsonl_path is not None:
            jsonl_sink += f" | tee -a {shlex.quote(combined_jsonl_path.as_posix())}"
        return (
            f"mkdir -p {shlex.quote(EnvironmentPaths.agent_dir.as_posix())} "
            f"{shlex.quote(self.rara_home.as_posix())} || exit $?; "
            "{ "
            f"{binary_invocation} exec --json --full-access "
            f"--runtime-profile {shlex.quote(self.runtime_profile)} --cwd {cwd} "
            f"--run-id {run_id} --task-id {task_id} "
            f"--output-last-message {quoted_last_message_path} - "
            f"< {quoted_instruction_path}; "
            f"printf '%s\\n' \"$?\" > {quoted_status_path}; "
            f"}} | {jsonl_sink}; "
            f"status=$(cat {quoted_status_path} 2>/dev/null || printf '1'); "
            'exit "$status"'
        )

    def _record_run(
        self,
        context: AgentContext,
        *,
        instruction: str,
        events: list[dict[str, Any]],
        verification_status: VerificationStatus,
    ) -> None:
        self._populate_context(context, events)
        context.metadata["verification_pass"] = self.verification_pass
        context.metadata["verification_status"] = verification_status.value
        context.metadata["verification_jsonl_path"] = (
            DEFAULT_VERIFICATION_JSONL_PATH.as_posix()
            if verification_status
            in {VerificationStatus.FAILED, VerificationStatus.COMPLETED}
            else None
        )
        self._write_trajectory(context, instruction=instruction, events=events)

    @staticmethod
    def _ca_certificate_install_command() -> str:
        bundle = shlex.quote(CA_CERTIFICATE_BUNDLE.as_posix())
        return (
            f"if [ -s {bundle} ]; then exit 0; fi; "
            "if command -v apt-get >/dev/null 2>&1; then "
            "export DEBIAN_FRONTEND=noninteractive; "
            "apt-get update && "
            "apt-get install -y --no-install-recommends ca-certificates && "
            "update-ca-certificates; "
            "elif command -v apk >/dev/null 2>&1; then "
            "apk add --no-cache ca-certificates && "
            "update-ca-certificates; "
            "elif command -v dnf >/dev/null 2>&1; then "
            "dnf install -y ca-certificates && "
            "(update-ca-trust extract || true); "
            "elif command -v yum >/dev/null 2>&1; then "
            "yum install -y ca-certificates && "
            "(update-ca-trust extract || true); "
            "else "
            "echo 'RARA requires CA certificates, but no supported package manager was found.' >&2; "
            "exit 1; "
            "fi"
        )

    def _build_rara_global_flags(self) -> str:
        flags: list[str] = []
        provider = self.effective_provider()
        if provider:
            flags.extend(["--provider", shlex.quote(provider)])
        if self.base_url:
            flags.extend(["--base-url", shlex.quote(self.base_url)])
        if self.model:
            flags.extend(["--model", shlex.quote(self.model)])
        if self.reasoning_effort:
            flags.extend(["--reasoning-effort", shlex.quote(self.reasoning_effort)])
        thinking = self.thinking
        if (
            thinking is None
            and self.reasoning_effort
            and self.effective_provider() == "deepseek"
        ):
            thinking = True
        if thinking is not None:
            flags.extend(["--thinking", str(thinking).lower()])
        return " ".join(flags)

    def _provider_env(self) -> dict[str, str]:
        api_key = self._api_key_from_host_env()
        if api_key is None:
            return {}
        return {"RARA_API_KEY": api_key}

    def _api_key_from_host_env(self) -> str | None:
        names: tuple[str, ...]
        if self.api_key_env:
            names = (self.api_key_env,)
        else:
            provider = self.effective_provider()
            names = PROVIDER_API_KEY_ENVS.get(provider or "", ())
        for name in names:
            value = os.environ.get(name)
            if value and value.strip():
                return value
        return None

    def effective_provider(self) -> str | None:
        if self.provider:
            return self.provider
        for provider in PROVIDER_INFERENCE_ORDER:
            if any(os.environ.get(name, "").strip() for name in PROVIDER_API_KEY_ENVS[provider]):
                return provider
        return None

    @staticmethod
    def _populate_context(context: AgentContext, events: list[dict[str, Any]]) -> None:
        final_message: str | None = None
        failure: str | None = None
        input_tokens = 0
        output_tokens = 0
        event_counts: dict[str, int] = {}
        runtime_profile: str | None = None

        for event in events:
            event_type = event.get("type")
            if isinstance(event_type, str):
                event_counts[event_type] = event_counts.get(event_type, 0) + 1
            if event_type == "thread.started":
                metadata = event.get("metadata") or {}
                if isinstance(metadata, dict):
                    runtime_profile = _string_value(metadata.get("runtime_profile"))
            elif event_type == "turn.completed":
                usage = event.get("usage") or {}
                u_input = usage.get("input_tokens")
                if u_input is not None:
                    input_tokens += int(u_input)
                u_output = usage.get("output_tokens")
                if u_output is not None:
                    output_tokens += int(u_output)
                if isinstance(event.get("final_message"), str):
                    final_message = event["final_message"]
            elif event_type == "turn.failed":
                error = event.get("error") or {}
                if isinstance(error.get("message"), str):
                    failure = error["message"]

        context.n_input_tokens = input_tokens
        context.n_output_tokens = output_tokens
        context.metadata = {
            "adapter": "rara-harbor",
            "event_count": len(events),
            "event_counts": event_counts,
            "final_message": final_message,
            "failure": failure,
            "jsonl_path": DEFAULT_JSONL_PATH.as_posix(),
            "last_message_path": DEFAULT_LAST_MESSAGE_PATH.as_posix(),
            "trajectory_path": DEFAULT_TRAJECTORY_PATH.as_posix(),
            "runtime_profile": runtime_profile,
        }

    def _write_trajectory(
        self,
        context: AgentContext,
        *,
        instruction: str,
        events: list[dict[str, Any]],
    ) -> None:
        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction=instruction,
            agent_version=self.version() or "unknown",
            default_model_name=self.model,
        )
        if trajectory is None:
            return

        trajectory_path = self.logs_dir / "trajectory.json"
        try:
            trajectory_path.write_text(
                format_trajectory_json(trajectory.to_json_dict()), encoding="utf-8"
            )
        except OSError as exc:
            self.logger.debug(f"Failed to write RARA trajectory file: {exc}")
            return

        if trajectory.final_metrics:
            metrics = trajectory.final_metrics
            context.cost_usd = metrics.total_cost_usd
            if metrics.total_prompt_tokens is not None:
                context.n_input_tokens = metrics.total_prompt_tokens
            if metrics.total_completion_tokens is not None:
                context.n_output_tokens = metrics.total_completion_tokens
            if metrics.total_cached_tokens is not None:
                context.n_cache_tokens = metrics.total_cached_tokens

    @staticmethod
    def _completed_with_mock_backend(events: list[dict[str, Any]]) -> bool:
        for event in events:
            if event.get("type") != "turn.completed":
                continue
            final_message = event.get("final_message")
            if isinstance(final_message, str) and final_message.startswith("Mock Response:"):
                return True
        return False

    def effective_cwd(self) -> str:
        return self.cwd or DEFAULT_BENCHMARK_CWD


def parse_rara_jsonl(output: str) -> list[dict[str, Any]]:
    """Extract RARA JSONL events from mixed stdout/stderr output."""
    events: list[dict[str, Any]] = []
    for line in output.splitlines():
        line = line.strip()
        if not line.startswith("{"):
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict) and isinstance(payload.get("type"), str):
            events.append(payload)
    return events


def convert_rara_events_to_trajectory(
    events: list[dict[str, Any]],
    *,
    instruction: str,
    agent_version: str = "unknown",
    default_model_name: str | None = None,
) -> Trajectory | None:
    """Convert RARA exec JSONL events into Harbor's ATIF trajectory model."""
    if not events and not instruction.strip():
        return None

    session_id = "unknown"
    run_id: str | None = None
    task_id: str | None = None
    runtime_profile: str | None = None
    event_counts: dict[str, int] = {}
    steps: list[Step] = []
    next_step_id = 1
    last_model_name = default_model_name
    pending_model_input: int | None = None
    pending_reasoning: list[str] = []
    pending_tool_calls: list[tuple[str, str]] = []
    tool_call_steps: dict[str, Step] = {}
    rara_sessions: list[dict[str, Any]] = []
    has_multiple_sessions = (
        sum(1 for event in events if event.get("type") == "thread.started") > 1
        or any(event.get("type") == "adapter.user_message" for event in events)
    )
    current_phase = "implementation"
    turn_step_start = 0
    total_input_tokens: int | None = None
    total_output_tokens: int | None = None
    final_message: str | None = None
    failure_message: str | None = None

    def append_step(**kwargs: Any) -> Step:
        nonlocal next_step_id
        step = Step(step_id=next_step_id, **kwargs)
        steps.append(step)
        next_step_id += 1
        return step

    if instruction.strip():
        append_step(source="user", message=instruction)

    for event in events:
        event_type = event.get("type")
        if isinstance(event_type, str):
            event_counts[event_type] = event_counts.get(event_type, 0) + 1

        timestamp = _string_value(event.get("timestamp"))

        if event_type == "thread.started":
            metadata = event.get("metadata") or {}
            if isinstance(metadata, dict):
                is_primary_session = not rara_sessions
                if not is_primary_session and current_phase == "implementation":
                    current_phase = "verification"
                pass_metadata = {
                    "phase": current_phase,
                    "session_id": _string_value(metadata.get("session_id")),
                    "run_id": _string_value(metadata.get("run_id")),
                    "task_id": _string_value(metadata.get("task_id")),
                    "runtime_profile": _string_value(
                        metadata.get("runtime_profile")
                    ),
                }
                rara_sessions.append(
                    {key: value for key, value in pass_metadata.items() if value is not None}
                )
                if is_primary_session:
                    session_id = pass_metadata["session_id"] or session_id
                    run_id = pass_metadata["run_id"]
                    task_id = pass_metadata["task_id"]
                    runtime_profile = pass_metadata["runtime_profile"]
            turn_step_start = len(steps)
            pending_model_input = None
            pending_reasoning.clear()
            pending_tool_calls.clear()
            tool_call_steps.clear()
            continue

        if event_type == "adapter.user_message":
            message = _string_value(event.get("message"))
            if message:
                append_step(source="user", timestamp=timestamp, message=message)
            current_phase = _string_value(event.get("phase")) or "verification"
            turn_step_start = len(steps)
            pending_model_input = None
            pending_reasoning.clear()
            pending_tool_calls.clear()
            tool_call_steps.clear()
            continue

        if event_type == "turn.started":
            turn_step_start = len(steps)
            pending_model_input = None
            pending_reasoning.clear()
            pending_tool_calls.clear()
            tool_call_steps.clear()
            continue

        if event_type == "turn.completed":
            usage = event.get("usage") or {}
            if isinstance(usage, dict):
                total_input_tokens = _add_optional_int(
                    total_input_tokens, _int_value(usage.get("input_tokens"))
                )
                total_output_tokens = _add_optional_int(
                    total_output_tokens, _int_value(usage.get("output_tokens"))
                )
            final_message = _string_value(event.get("final_message"))
            if final_message and not _has_agent_message_step(
                steps[turn_step_start:], final_message
            ):
                append_step(
                    source="agent",
                    timestamp=timestamp,
                    model_name=last_model_name,
                    message=final_message,
                )
            turn_step_start = len(steps)
            pending_model_input = None
            pending_reasoning.clear()
            pending_tool_calls.clear()
            tool_call_steps.clear()
            continue

        if event_type == "turn.failed":
            error = event.get("error") or {}
            if isinstance(error, dict):
                failure_message = _string_value(error.get("message"))
            if failure_message:
                append_step(
                    source="system",
                    timestamp=timestamp,
                    message=f"RARA exec failed: {failure_message}",
                )
            turn_step_start = len(steps)
            pending_model_input = None
            pending_reasoning.clear()
            pending_tool_calls.clear()
            tool_call_steps.clear()
            continue

        if event_type != "item.completed":
            continue

        item = event.get("item") or {}
        if not isinstance(item, dict):
            continue
        raw_item_id = _string_value(item.get("id")) or f"item_{next_step_id}"
        item_id = (
            f"{current_phase}:{raw_item_id}"
            if has_multiple_sessions
            else raw_item_id
        )
        item_type = item.get("type")

        if item_type == "reasoning":
            text = _string_value(item.get("text"))
            if text:
                pending_reasoning.append(text)
            continue

        if item_type == "agent_message":
            text = _string_value(item.get("text"))
            if text:
                append_step(
                    source="agent",
                    timestamp=timestamp,
                    model_name=last_model_name,
                    message=text,
                    reasoning_content=_take_joined(pending_reasoning),
                )
            continue

        if item_type == "tool_call":
            name = _string_value(item.get("name")) or "tool"
            arguments = item.get("input")
            if not isinstance(arguments, dict):
                arguments = {"input": arguments}
            pending_tool_calls.append((item_id, name))
            tool_call_steps[item_id] = append_step(
                source="agent",
                timestamp=timestamp,
                model_name=last_model_name,
                message=f"Call tool `{name}`.",
                reasoning_content=_take_joined(pending_reasoning),
                tool_calls=[
                    ToolCall(
                        tool_call_id=item_id,
                        function_name=name,
                        arguments=arguments,
                    )
                ],
            )
            continue

        if item_type == "tool_result":
            name = _string_value(item.get("name")) or "tool"
            content = _string_value(item.get("content")) or ""
            is_error = bool(item.get("is_error"))
            call_id = _pop_matching_tool_call(pending_tool_calls, name)
            _append_observation(
                steps,
                tool_call_steps=tool_call_steps,
                source_call_id=call_id,
                content=content,
                extra={"tool_name": name, "is_error": is_error},
            )
            continue

        if item_type == "tool_progress":
            name = _string_value(item.get("name")) or "tool"
            stream = _string_value(item.get("stream"))
            chunk = _string_value(item.get("chunk")) or ""
            call_id = _first_matching_tool_call(pending_tool_calls, name)
            _append_observation(
                steps,
                tool_call_steps=tool_call_steps,
                source_call_id=call_id,
                content=chunk,
                extra={"tool_name": name, "stream": stream, "progress": True},
            )
            continue

        if item_type == "model_request":
            last_model_name = _string_value(item.get("model")) or last_model_name
            pending_model_input = _int_value(item.get("input_tokens"))
            continue

        if item_type == "model_response":
            last_model_name = _string_value(item.get("model")) or last_model_name
            output_tokens = _int_value(item.get("output_tokens"))
            _attach_metrics_to_latest_agent_step(
                steps,
                Metrics(
                    prompt_tokens=pending_model_input,
                    completion_tokens=output_tokens,
                    extra={"finish_reason": _string_value(item.get("finish_reason"))},
                ),
            )
            pending_model_input = None
            continue

        if item_type == "status":
            message = _string_value(item.get("message"))
            if message:
                append_step(source="system", timestamp=timestamp, message=message)
            continue

        if item_type == "error":
            message = _string_value(item.get("message"))
            if message:
                append_step(
                    source="system",
                    timestamp=timestamp,
                    message=message,
                    extra={"recoverable": bool(item.get("recoverable"))},
                )
            continue

    if not steps:
        return None

    final_extra = {
        "event_counts": event_counts,
        "run_id": run_id,
        "task_id": task_id,
        "runtime_profile": runtime_profile,
        "rara_sessions": rara_sessions or None,
        "final_message": final_message,
        "failure": failure_message,
    }

    return Trajectory(
        schema_version="ATIF-v1.7",
        session_id=session_id,
        agent=Agent(
            name="rara",
            version=agent_version,
            model_name=last_model_name,
        ),
        steps=steps,
        final_metrics=FinalMetrics(
            total_prompt_tokens=total_input_tokens,
            total_completion_tokens=total_output_tokens,
            total_cached_tokens=None,
            total_cost_usd=None,
            total_steps=len(steps),
            extra={k: v for k, v in final_extra.items() if v is not None},
        ),
    )


def _string_value(value: Any) -> str | None:
    return value if isinstance(value, str) else None


def _int_value(value: Any) -> int | None:
    return value if isinstance(value, int) else None


def _add_optional_int(total: int | None, value: int | None) -> int | None:
    if value is None:
        return total
    return (total or 0) + value


def _take_joined(parts: list[str]) -> str | None:
    if not parts:
        return None
    joined = "\n".join(parts)
    parts.clear()
    return joined


def _has_agent_message_step(steps: list[Step], message: str) -> bool:
    return any(step.source == "agent" and step.message == message for step in steps)


def _first_matching_tool_call(pending: list[tuple[str, str]], name: str) -> str | None:
    for call_id, pending_name in pending:
        if pending_name == name:
            return call_id
    return None


def _pop_matching_tool_call(pending: list[tuple[str, str]], name: str) -> str | None:
    for index, (call_id, pending_name) in enumerate(pending):
        if pending_name == name:
            del pending[index]
            return call_id
    return None


def _append_observation(
    steps: list[Step],
    *,
    tool_call_steps: dict[str, Step],
    source_call_id: str | None,
    content: str,
    extra: dict[str, Any],
) -> None:
    result = ObservationResult(
        source_call_id=source_call_id,
        content=content,
        extra={k: v for k, v in extra.items() if v is not None},
    )
    step = tool_call_steps.get(source_call_id) if source_call_id else None
    if step is None:
        step = next((step for step in reversed(steps) if step.source == "agent"), None)
    if step is None:
        return
    if step.observation is None:
        step.observation = Observation(results=[result])
    else:
        step.observation.results.append(result)


def _attach_metrics_to_latest_agent_step(steps: list[Step], metrics: Metrics) -> None:
    for step in reversed(steps):
        if step.source == "agent":
            if step.metrics is None:
                step.metrics = metrics
            return
