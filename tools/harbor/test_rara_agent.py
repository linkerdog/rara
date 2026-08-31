from __future__ import annotations

import asyncio
import json
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import UUID

from harbor.models.agent.context import AgentContext

from rara_agent import (
    RaraAgent,
    VerificationStatus,
    parse_rara_jsonl,
)


class FakeExecResult:
    def __init__(
        self, return_code: int, stdout: str | None = "", stderr: str | None = ""
    ) -> None:
        self.return_code = return_code
        self.stdout = stdout
        self.stderr = stderr


class FakeInstallEnvironment:
    def __init__(self) -> None:
        self.uploads: list[tuple[Path, str]] = []
        self.commands: list[str] = []

    async def upload_file(self, source: Path, destination: str) -> None:
        self.uploads.append((source, destination))

    async def exec(self, command: str, **_: object) -> FakeExecResult:
        self.commands.append(command)
        if command.endswith("--version"):
            return FakeExecResult(
                126,
                stdout="cannot execute binary file: Exec format error",
            )
        return FakeExecResult(0)


class FakeRunEnvironment:
    def __init__(self) -> None:
        self.uploads: list[tuple[Path, str]] = []
        self.exec_env: dict[str, str] | None = None
        self.exec_cwd: str | None = None
        self.commands: list[str] = []

    async def upload_file(self, source: Path, destination: str) -> None:
        self.uploads.append((source, destination))

    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
    ) -> FakeExecResult:
        self.exec_cwd = cwd
        self.exec_env = dict(env or {})
        self.commands.append(command)
        return FakeExecResult(
            0,
            stdout='{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1},"final_message":"done"}\n',
        )


class FakeTrajectoryEnvironment(FakeRunEnvironment):
    async def exec(
        self,
        command: str,
        cwd: str | None = None,
        env: dict[str, str] | None = None,
    ) -> FakeExecResult:
        self.exec_cwd = cwd
        self.exec_env = dict(env or {})
        self.commands.append(command)
        events = [
            {
                "type": "thread.started",
                "metadata": {
                    "session_id": "session-1",
                    "run_id": "run-1",
                    "task_id": "task-1",
                    "runtime_profile": "headless-coding-v1",
                },
                "timestamp": "2026-07-11T00:00:00Z",
            },
            {"type": "turn.started", "timestamp": "2026-07-11T00:00:01Z"},
            {
                "type": "item.completed",
                "item": {
                    "id": "item_0",
                    "type": "model_request",
                    "model": "gemini-2.5-flash",
                    "input_tokens": 11,
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_1",
                    "type": "reasoning",
                    "text": "Need to inspect files.",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_2",
                    "type": "tool_call",
                    "name": "bash",
                    "input": {"command": "ls"},
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_3",
                    "type": "tool_progress",
                    "name": "bash",
                    "stream": "stdout",
                    "chunk": "solution.sparql\n",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_4",
                    "type": "tool_result",
                    "name": "bash",
                    "content": "done",
                    "is_error": False,
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_5",
                    "type": "agent_message",
                    "text": "Created /app/solution.sparql.",
                },
            },
            {
                "type": "item.completed",
                "item": {
                    "id": "item_6",
                    "type": "model_response",
                    "model": "gemini-2.5-flash",
                    "output_tokens": 7,
                    "finish_reason": "stop",
                },
            },
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 13, "output_tokens": 9},
                "final_message": "Created /app/solution.sparql.",
                "timestamp": "2026-07-11T00:00:02Z",
            },
        ]
        return FakeExecResult(0, stdout="\n".join(json_line(event) for event in events))


def json_line(value: dict[str, object]) -> str:
    return json.dumps(value)


class RaraAgentTests(unittest.TestCase):
    def test_agent_declares_atif_support(self) -> None:
        self.assertTrue(RaraAgent.SUPPORTS_ATIF)

    def test_parse_rara_jsonl_ignores_non_json_lines(self) -> None:
        output = "\n".join(
            [
                "warning: setup detail",
                '{"type":"thread.started","metadata":{"session_id":"s"},"timestamp":"t"}',
                "not json",
                '{"type":"turn.completed","usage":{"input_tokens":3,"output_tokens":2},"final_message":"done","timestamp":"t"}',
            ]
        )

        events = parse_rara_jsonl(output)

        self.assertEqual(
            [event["type"] for event in events], ["thread.started", "turn.completed"]
        )

    def test_populate_context_reads_usage_and_final_message(self) -> None:
        context = AgentContext()
        events = [
            {"type": "thread.started"},
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 13, "output_tokens": 7},
                "final_message": "done",
            },
        ]

        RaraAgent._populate_context(context, events)

        self.assertEqual(context.n_input_tokens, 13)
        self.assertEqual(context.n_output_tokens, 7)
        self.assertEqual(context.metadata["final_message"], "done")
        self.assertEqual(context.metadata["event_counts"]["turn.completed"], 1)
        self.assertEqual(context.metadata["trajectory_path"], "/logs/agent/trajectory.json")

    def test_populate_context_preserves_zero_token_counts(self) -> None:
        context = AgentContext()
        context.n_input_tokens = 9
        context.n_output_tokens = 9
        events = [
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 0, "output_tokens": 0},
            },
        ]

        RaraAgent._populate_context(context, events)

        self.assertEqual(context.n_input_tokens, 0)
        self.assertEqual(context.n_output_tokens, 0)

    def test_populate_context_accumulates_multiple_passes(self) -> None:
        context = AgentContext()
        events = [
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 5, "output_tokens": 2},
                "final_message": "implemented",
            },
            {
                "type": "turn.completed",
                "usage": {"input_tokens": 7, "output_tokens": 3},
                "final_message": "verified",
            },
        ]

        RaraAgent._populate_context(context, events)

        self.assertEqual(context.n_input_tokens, 12)
        self.assertEqual(context.n_output_tokens, 5)
        self.assertEqual(context.metadata["final_message"], "verified")

    def test_build_exec_command_uses_context_and_session_ids(self) -> None:
        agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            remote_binary="/opt/rara",
            rara_home="/tmp/rara-home",
        )
        agent.context_id = UUID("00000000-0000-0000-0000-000000000123")
        agent.session_id = "trial-agent"

        with patch.dict("os.environ", {}, clear=True):
            command = agent._build_exec_command()

        self.assertIn("/opt/rara exec --json --full-access", command)
        self.assertIn("--runtime-profile headless-coding-v1", command)
        self.assertIn("--cwd /app", command)
        self.assertIn("--run-id 00000000000000000000000000000123", command)
        self.assertIn("--task-id trial-agent", command)
        self.assertIn("--output-last-message /logs/agent/last-message.txt", command)
        self.assertIn("< /logs/agent/instruction.txt", command)
        self.assertIn("{ /opt/rara exec --json --full-access", command)
        self.assertIn("printf '%s\\n' \"$?\" > /logs/agent/rara-exec.status", command)
        self.assertIn("status=$(cat /logs/agent/rara-exec.status", command)
        self.assertIn('exit "$status"', command)
        self.assertNotIn("2>&1", command)
        self.assertIn("| tee /logs/agent/rara-exec.jsonl", command)

    def test_build_exec_command_includes_provider_flags_without_api_key(self) -> None:
        agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            remote_binary="/opt/rara",
            provider="gemini",
            model="gemini-2.5-flash",
        )

        command = agent._build_exec_command()

        self.assertIn(
            "/opt/rara --provider gemini --model gemini-2.5-flash exec --json --full-access",
            command,
        )
        self.assertNotIn("--api-key", command)

    def test_build_exec_command_enables_deepseek_thinking_for_reasoning_effort(self) -> None:
        agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            remote_binary="/opt/rara",
            provider="deepseek",
            model="deepseek-v4-pro",
            reasoning_effort="high",
        )

        command = agent._build_exec_command()

        self.assertIn("--reasoning-effort high", command)
        self.assertIn("--thinking true", command)

    def test_explicit_thinking_override_wins_over_deepseek_inference(self) -> None:
        agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            provider="deepseek",
            reasoning_effort="high",
            thinking="false",
        )

        flags = agent._build_rara_global_flags()

        self.assertIn("--reasoning-effort high", flags)
        self.assertIn("--thinking false", flags)

    def test_verification_pass_defaults_on_and_accepts_explicit_false(self) -> None:
        default_agent = RaraAgent(logs_dir=Path("/tmp/logs"), binary_path="/tmp/rara")
        disabled_agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            verification_pass="false",
        )

        self.assertTrue(default_agent.verification_pass)
        self.assertFalse(disabled_agent.verification_pass)

    def test_default_cwd_matches_terminal_bench_workdir(self) -> None:
        agent = RaraAgent(logs_dir=Path("/tmp/logs"), binary_path="/tmp/rara")

        self.assertEqual(agent.cwd, "/app")

    def test_empty_cwd_falls_back_to_terminal_bench_workdir(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara", cwd="")
            context = AgentContext()
            environment = FakeRunEnvironment()

            asyncio.run(agent.run("Create /app/output.txt.", environment, context))  # type: ignore[arg-type]

            uploaded_instruction = environment.uploads[0][0].read_text(encoding="utf-8")

        self.assertEqual(agent.effective_cwd(), "/app")
        self.assertEqual(environment.exec_cwd, "/app")
        self.assertIn("--cwd /app", agent._build_exec_command())
        self.assertIn("Work only in the benchmark workspace: /app", uploaded_instruction)

    def test_run_sets_home_and_cwd_for_benchmark(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeRunEnvironment()

            asyncio.run(agent.run("solve task", environment, context))  # type: ignore[arg-type]

        self.assertEqual(environment.exec_env["RARA_HOME"], "/logs/agent/rara-home")
        self.assertEqual(environment.exec_cwd, "/app")

    def test_run_writes_atif_trajectory(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(
                logs_dir=Path(temp),
                binary_path="/tmp/rara",
                verification_pass=False,
            )
            context = AgentContext()
            environment = FakeTrajectoryEnvironment()

            asyncio.run(agent.run("Create /app/solution.sparql.", environment, context))  # type: ignore[arg-type]

            trajectory = Path(temp, "trajectory.json")
            data = trajectory.read_text(encoding="utf-8")

        self.assertIn('"schema_version": "ATIF-v1.7"', data)
        self.assertIn('"session_id": "session-1"', data)
        self.assertIn('"name": "rara"', data)
        self.assertIn('"function_name": "bash"', data)
        self.assertIn('"source_call_id": "item_2"', data)
        self.assertIn('"runtime_profile": "headless-coding-v1"', data)
        self.assertEqual(context.n_input_tokens, 13)
        self.assertEqual(context.n_output_tokens, 9)

    def test_run_executes_independent_verification_and_aggregates_usage(self) -> None:
        class FakeTwoPassEnvironment(FakeRunEnvironment):
            async def exec(
                self,
                command: str,
                cwd: str | None = None,
                env: dict[str, str] | None = None,
            ) -> FakeExecResult:
                self.exec_cwd = cwd
                self.exec_env = dict(env or {})
                self.commands.append(command)
                is_implementation = len(self.commands) == 1
                session_id = (
                    "implementation-session"
                    if is_implementation
                    else "verification-session"
                )
                task_id = (
                    "harbor-task"
                    if is_implementation
                    else "harbor-task-verification"
                )
                usage = (
                    {"input_tokens": 5, "output_tokens": 2}
                    if is_implementation
                    else {"input_tokens": 7, "output_tokens": 3}
                )
                return FakeExecResult(
                    0,
                    stdout="\n".join(
                        json_line(event)
                        for event in [
                            {
                                "type": "thread.started",
                                "metadata": {
                                    "session_id": session_id,
                                    "run_id": "harbor-run",
                                    "task_id": task_id,
                                    "runtime_profile": "headless-coding-v1",
                                },
                            },
                            {"type": "turn.started"},
                            {
                                "type": "item.completed",
                                "item": {
                                    "id": "call_0",
                                    "type": "tool_call",
                                    "name": "bash",
                                    "input": {"command": task_id},
                                },
                            },
                            {
                                "type": "item.completed",
                                "item": {
                                    "id": "result_0",
                                    "type": "tool_result",
                                    "name": "bash",
                                    "content": task_id,
                                    "is_error": False,
                                },
                            },
                            {
                                "type": "turn.completed",
                                "usage": usage,
                                "final_message": "completed",
                            },
                        ]
                    ),
                )

        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeTwoPassEnvironment()

            asyncio.run(agent.run("Implement /app/tool.py.", environment, context))  # type: ignore[arg-type]

            verification_instruction = Path(
                temp, "verification-instruction.txt"
            ).read_text(encoding="utf-8")
            trajectory = json.loads(
                Path(temp, "trajectory.json").read_text(encoding="utf-8")
            )

        self.assertEqual(len(environment.commands), 2)
        self.assertIn(
            "--output-last-message /logs/agent/implementation-last-message.txt",
            environment.commands[0],
        )
        self.assertIn("< /logs/agent/verification-instruction.txt", environment.commands[1])
        self.assertIn("| tee /logs/agent/rara-verification.jsonl", environment.commands[1])
        self.assertIn("| tee -a /logs/agent/rara-exec.jsonl", environment.commands[1])
        self.assertIn("--task-id harbor-task-verification", environment.commands[1])
        self.assertIn("independent final verification and repair pass", verification_instruction)
        self.assertIn("Original task:\nImplement /app/tool.py.", verification_instruction)
        self.assertIn(
            "Reported implementation and validation evidence:\ncompleted",
            verification_instruction,
        )
        self.assertIn("Do not repeat already evidenced checks", verification_instruction)
        self.assertIn("missing from the reported evidence", verification_instruction)
        self.assertIn("background or detached child behavior", verification_instruction)
        self.assertEqual(trajectory["session_id"], "implementation-session")
        sessions = trajectory["final_metrics"]["extra"]["rara_sessions"]
        self.assertEqual(
            [session["phase"] for session in sessions],
            ["implementation", "verification"],
        )
        self.assertEqual(
            [session["session_id"] for session in sessions],
            ["implementation-session", "verification-session"],
        )
        self.assertEqual(
            [session["task_id"] for session in sessions],
            ["harbor-task", "harbor-task-verification"],
        )
        agent_messages = [
            step.get("message")
            for step in trajectory["steps"]
            if step["source"] == "agent"
        ]
        self.assertEqual(agent_messages.count("completed"), 2)
        tool_steps = [step for step in trajectory["steps"] if step.get("tool_calls")]
        self.assertEqual(
            [step["tool_calls"][0]["tool_call_id"] for step in tool_steps],
            ["implementation:call_0", "verification:call_0"],
        )
        self.assertEqual(
            [step["observation"]["results"][0]["content"] for step in tool_steps],
            ["harbor-task", "harbor-task-verification"],
        )
        self.assertEqual(
            [
                step["observation"]["results"][0]["source_call_id"]
                for step in tool_steps
            ],
            ["implementation:call_0", "verification:call_0"],
        )
        self.assertEqual(context.n_input_tokens, 12)
        self.assertEqual(context.n_output_tokens, 5)
        self.assertEqual(context.metadata["final_message"], "completed")
        self.assertEqual(context.metadata["verification_status"], "completed")

    def test_run_skips_verification_when_explicitly_disabled(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(
                logs_dir=Path(temp),
                binary_path="/tmp/rara",
                verification_pass=False,
            )
            context = AgentContext()
            environment = FakeRunEnvironment()

            asyncio.run(agent.run("Implement /app/tool.py.", environment, context))  # type: ignore[arg-type]

        self.assertEqual(len(environment.commands), 1)
        self.assertNotIn("verification-instruction.txt", environment.commands[0])
        self.assertFalse(context.metadata["verification_pass"])
        self.assertEqual(context.metadata["verification_status"], "disabled")
        self.assertIsNone(context.metadata["verification_jsonl_path"])

    def test_run_does_not_verify_after_implementation_failure(self) -> None:
        class FakeFailedEnvironment(FakeRunEnvironment):
            async def exec(
                self,
                command: str,
                cwd: str | None = None,
                env: dict[str, str] | None = None,
            ) -> FakeExecResult:
                self.exec_cwd = cwd
                self.exec_env = dict(env or {})
                self.commands.append(command)
                return FakeExecResult(
                    2,
                    stdout=json_line(
                        {
                            "type": "turn.failed",
                            "error": {"message": "implementation failed"},
                        }
                    ),
                    stderr="implementation failed",
                )

        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeFailedEnvironment()

            with self.assertRaises(RuntimeError):
                asyncio.run(agent.run("Implement /app/tool.py.", environment, context))  # type: ignore[arg-type]

        self.assertEqual(len(environment.commands), 1)
        self.assertEqual(context.metadata["failure"], "implementation failed")
        self.assertEqual(context.metadata["verification_status"], "not_started")
        self.assertIsNone(context.metadata["verification_jsonl_path"])

    def test_record_run_exposes_verification_artifact_after_failure(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            agent._record_run(
                context,
                instruction="Verify /app/tool.py.",
                events=[],
                verification_status=VerificationStatus.FAILED,
            )

        self.assertEqual(context.metadata["verification_status"], "failed")
        self.assertEqual(
            context.metadata["verification_jsonl_path"],
            "/logs/agent/rara-verification.jsonl",
        )

    def test_write_trajectory_preserves_zero_token_context_metrics(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            context.n_input_tokens = 9
            context.n_output_tokens = 9
            events = parse_rara_jsonl(
                '{"type":"turn.completed","usage":{"input_tokens":0,"output_tokens":0},"final_message":"done"}'
            )

            agent._write_trajectory(context, instruction="Finish.", events=events)

        self.assertEqual(context.n_input_tokens, 0)
        self.assertEqual(context.n_output_tokens, 0)

    def test_run_maps_inferred_provider_api_key_to_rara_api_key(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeRunEnvironment()

            with patch.dict("os.environ", {"GEMINI_API_KEY": "secret"}, clear=True):
                asyncio.run(agent.run("solve task", environment, context))  # type: ignore[arg-type]
                self.assertEqual(agent.effective_provider(), "gemini")
                self.assertIn("--provider gemini", agent._build_exec_command())

        self.assertEqual(environment.exec_env["RARA_API_KEY"], "secret")

    def test_run_prefers_explicit_extra_env_over_inferred_provider_key(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(
                logs_dir=Path(temp),
                binary_path="/tmp/rara",
                extra_env={"RARA_API_KEY": "explicit"},
            )
            context = AgentContext()
            environment = FakeRunEnvironment()

            with patch.dict("os.environ", {"GEMINI_API_KEY": "inferred"}, clear=True):
                asyncio.run(agent.run("solve task", environment, context))  # type: ignore[arg-type]

        self.assertEqual(environment.exec_env["RARA_API_KEY"], "explicit")

    def test_run_rejects_mock_backend_completion(self) -> None:
        class FakeMockRunEnvironment(FakeRunEnvironment):
            async def exec(
                self,
                command: str,
                cwd: str | None = None,
                env: dict[str, str] | None = None,
            ) -> FakeExecResult:
                self.exec_cwd = cwd
                self.exec_env = dict(env or {})
                return FakeExecResult(
                    0,
                    stdout='{"type":"turn.completed","final_message":"Mock Response: solve task"}\n',
                )

        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeMockRunEnvironment()

            with self.assertRaisesRegex(RuntimeError, "mock backend"):
                asyncio.run(agent.run("solve task", environment, context))  # type: ignore[arg-type]

    def test_run_wraps_instruction_with_benchmark_artifact_guidance(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeRunEnvironment()

            asyncio.run(
                agent.run(
                    "Save your query in `/app/solution.sparql`.",
                    environment,
                    context,
                )
            )  # type: ignore[arg-type]

            uploaded_instruction = environment.uploads[0][0].read_text(encoding="utf-8")

        self.assertIn("non-interactive Terminal-Bench task container", uploaded_instruction)
        self.assertIn("Work only in the benchmark workspace: /app", uploaded_instruction)
        self.assertIn("create every file path that the task asks for", uploaded_instruction)
        self.assertIn("Prefer dedicated file tools over shell commands", uploaded_instruction)
        self.assertIn(
            "use apply_patch or write_file for file modifications",
            uploaded_instruction,
        )
        self.assertIn("Do not use shell redirection, heredocs, sed, awk, perl", uploaded_instruction)
        self.assertIn("Use shell commands for process execution", uploaded_instruction)
        self.assertIn("run_in_background for long-running", uploaded_instruction)
        self.assertIn("Use PTY tools only", uploaded_instruction)
        self.assertIn("short validation checklist", uploaded_instruction)
        self.assertIn("through the artifact's public interface", uploaded_instruction)
        self.assertIn("poll readiness", uploaded_instruction)
        self.assertIn("separate client or process", uploaded_instruction)
        self.assertIn("Do not infer success from launch output", uploaded_instruction)
        self.assertIn("Treat task constraints as validation requirements", uploaded_instruction)
        self.assertIn("substitutions must come from an allowed list", uploaded_instruction)
        self.assertIn("fresh non-interactive process", uploaded_instruction)
        self.assertIn("shell startup files alone does not prove", uploaded_instruction)
        self.assertIn("smallest implementation that satisfies", uploaded_instruction)
        self.assertIn("move to uncovered checklist items", uploaded_instruction)
        self.assertIn("compare the implementation and validation evidence", uploaded_instruction)
        self.assertIn("request escalated sandbox permissions", uploaded_instruction)
        self.assertIn("/app/solution.sparql", uploaded_instruction)
        self.assertIn("Do not finish with only an explanation", uploaded_instruction)
        self.assertIn("Completion gate:", uploaded_instruction)
        self.assertIn("Before your final answer, re-read the task", uploaded_instruction)
        self.assertIn("every applicable interaction and lifecycle mode", uploaded_instruction)
        self.assertIn("Long-running or detached work", uploaded_instruction)
        self.assertIn("keep working or report it as unverified", uploaded_instruction)

    def test_binary_path_is_resolved(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            binary_path = Path(temp) / "rara"
            binary_path.touch()
            relative_path = binary_path.relative_to(Path.cwd())

            agent = RaraAgent(logs_dir=Path("/tmp/logs"), binary_path=str(relative_path))

        self.assertTrue(agent.binary_path.is_absolute())

    def test_install_reports_remote_binary_validation_failure(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            binary_path = Path(temp) / "rara"
            binary_path.touch()
            agent = RaraAgent(logs_dir=Path("/tmp/logs"), binary_path=str(binary_path))
            environment = FakeInstallEnvironment()

            with self.assertRaisesRegex(RuntimeError, "Linux binary"):
                asyncio.run(agent.install(environment))  # type: ignore[arg-type]

        self.assertIn("/installed-agent/rara --version", environment.commands)

    def test_install_ensures_ca_certificates_before_validation(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            binary_path = Path(temp) / "rara"
            binary_path.touch()
            agent = RaraAgent(logs_dir=Path("/tmp/logs"), binary_path=str(binary_path))
            environment = FakeInstallEnvironment()

            with self.assertRaisesRegex(RuntimeError, "Linux binary"):
                asyncio.run(agent.install(environment))  # type: ignore[arg-type]

        ca_command_index = next(
            index
            for index, command in enumerate(environment.commands)
            if "ca-certificates" in command
        )
        validation_index = next(
            index
            for index, command in enumerate(environment.commands)
            if command.endswith("--version")
        )
        self.assertLess(ca_command_index, validation_index)

    def test_ca_certificate_install_command_supports_common_images(self) -> None:
        command = RaraAgent._ca_certificate_install_command()

        self.assertIn("/etc/ssl/certs/ca-certificates.crt", command)
        self.assertIn(
            "apt-get update && "
            "apt-get install -y --no-install-recommends ca-certificates && "
            "update-ca-certificates",
            command,
        )
        self.assertIn(
            "apk add --no-cache ca-certificates && update-ca-certificates", command
        )
        self.assertIn(
            "dnf install -y ca-certificates && (update-ca-trust extract || true)",
            command,
        )
        self.assertIn(
            "yum install -y ca-certificates && (update-ca-trust extract || true)",
            command,
        )


if __name__ == "__main__":
    unittest.main()
