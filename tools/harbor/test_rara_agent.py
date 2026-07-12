from __future__ import annotations

import asyncio
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import UUID

from harbor.models.agent.context import AgentContext

from rara_agent import (
    RaraAgent,
    build_benchmark_instruction,
    convert_rara_events_to_trajectory,
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
        events = [
            {
                "type": "thread.started",
                "metadata": {
                    "session_id": "session-1",
                    "run_id": "run-1",
                    "task_id": "task-1",
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
    import json

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

    def test_run_disables_local_embeddings_for_benchmark(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
            context = AgentContext()
            environment = FakeRunEnvironment()

            asyncio.run(agent.run("solve task", environment, context))  # type: ignore[arg-type]

        self.assertEqual(environment.exec_env["RARA_LOCAL_EMBEDDINGS"], "off")
        self.assertEqual(environment.exec_env["RARA_HOME"], "/logs/agent/rara-home")
        self.assertEqual(environment.exec_cwd, "/app")

    def test_run_writes_atif_trajectory(self) -> None:
        with tempfile.TemporaryDirectory(dir=Path.cwd()) as temp:
            agent = RaraAgent(logs_dir=Path(temp), binary_path="/tmp/rara")
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
        self.assertEqual(context.n_input_tokens, 13)
        self.assertEqual(context.n_output_tokens, 9)

    def test_convert_rara_events_to_trajectory_links_tool_observations(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"thread.started","metadata":{"session_id":"s"},"timestamp":"2026-07-11T00:00:00Z"}',
                    '{"type":"item.completed","item":{"id":"call_1","type":"tool_call","name":"bash","input":{"command":"true"}}}',
                    '{"type":"item.completed","item":{"id":"result_1","type":"tool_result","name":"bash","content":"ok","is_error":false}}',
                    '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2},"final_message":"done","timestamp":"2026-07-11T00:00:01Z"}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Run a command.",
            agent_version="test",
            default_model_name="mock",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        tool_step = next(step for step in trajectory.steps if step.tool_calls)
        self.assertEqual(tool_step.tool_calls[0].tool_call_id, "call_1")
        self.assertIsNotNone(tool_step.observation)
        assert tool_step.observation is not None
        self.assertEqual(tool_step.observation.results[0].source_call_id, "call_1")
        self.assertEqual(tool_step.observation.results[0].content, "ok")
        self.assertEqual(trajectory.final_metrics.total_prompt_tokens, 5)
        self.assertEqual(trajectory.final_metrics.total_completion_tokens, 2)

    def test_convert_rara_events_to_trajectory_keeps_same_name_results_with_calls(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"item.completed","item":{"id":"call_1","type":"tool_call","name":"read_file","input":{"path":"/app/main.tex"}}}',
                    '{"type":"item.completed","item":{"id":"call_2","type":"tool_call","name":"read_file","input":{"path":"/app/input.tex"}}}',
                    '{"type":"item.completed","item":{"id":"call_3","type":"tool_call","name":"read_file","input":{"path":"/app/synonyms.txt"}}}',
                    '{"type":"item.completed","item":{"id":"progress_1","type":"tool_progress","name":"read_file","stream":"stdout","chunk":"main progress"}}',
                    '{"type":"item.completed","item":{"id":"result_1","type":"tool_result","name":"read_file","content":"main result","is_error":false}}',
                    '{"type":"item.completed","item":{"id":"result_2","type":"tool_result","name":"read_file","content":"input result","is_error":false}}',
                    '{"type":"item.completed","item":{"id":"result_3","type":"tool_result","name":"read_file","content":"synonyms result","is_error":false}}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Read the input files.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        tool_steps = [step for step in trajectory.steps if step.tool_calls]
        self.assertEqual(
            [step.tool_calls[0].tool_call_id for step in tool_steps],
            ["call_1", "call_2", "call_3"],
        )
        self.assertEqual(
            [
                [(result.source_call_id, result.content) for result in step.observation.results]
                for step in tool_steps
            ],
            [
                [("call_1", "main progress"), ("call_1", "main result")],
                [("call_2", "input result")],
                [("call_3", "synonyms result")],
            ],
        )

    def test_convert_rara_events_to_trajectory_drops_orphaned_calls_at_turn_boundaries(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"item.completed","item":{"id":"completed_orphan","type":"tool_call","name":"read_file","input":{"path":"/app/orphan-after-completion"}}}',
                    '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1},"final_message":"first"}',
                    '{"type":"item.completed","item":{"id":"after_completion","type":"tool_call","name":"read_file","input":{"path":"/app/after-completion"}}}',
                    '{"type":"item.completed","item":{"id":"result_after_completion","type":"tool_result","name":"read_file","content":"after completion","is_error":false}}',
                    '{"type":"item.completed","item":{"id":"failed_orphan","type":"tool_call","name":"read_file","input":{"path":"/app/orphan-after-failure"}}}',
                    '{"type":"turn.failed","error":{"message":"interrupted"}}',
                    '{"type":"item.completed","item":{"id":"after_failure","type":"tool_call","name":"read_file","input":{"path":"/app/after-failure"}}}',
                    '{"type":"item.completed","item":{"id":"result_after_failure","type":"tool_result","name":"read_file","content":"after failure","is_error":false}}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Read the files.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        tool_steps = {
            step.tool_calls[0].tool_call_id: step
            for step in trajectory.steps
            if step.tool_calls
        }
        self.assertIsNone(tool_steps["completed_orphan"].observation)
        self.assertEqual(
            tool_steps["after_completion"].observation.results[0].source_call_id,
            "after_completion",
        )
        self.assertIsNone(tool_steps["failed_orphan"].observation)
        self.assertEqual(
            tool_steps["after_failure"].observation.results[0].source_call_id,
            "after_failure",
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

    def test_convert_rara_events_to_trajectory_accumulates_turn_usage(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"turn.completed","usage":{"input_tokens":5,"output_tokens":2},"final_message":"first"}',
                    '{"type":"turn.completed","usage":{"input_tokens":7,"output_tokens":3},"final_message":"second"}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Run multiple turns.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        self.assertEqual(trajectory.final_metrics.total_prompt_tokens, 12)
        self.assertEqual(trajectory.final_metrics.total_completion_tokens, 5)

    def test_unmatched_tool_result_does_not_borrow_other_tool_call_id(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"item.completed","item":{"id":"call_1","type":"tool_call","name":"read_file","input":{"path":"/app/a"}}}',
                    '{"type":"item.completed","item":{"id":"result_1","type":"tool_result","name":"bash","content":"ok","is_error":false}}',
                    '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1},"final_message":"done"}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Run one tool.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        tool_step = next(step for step in trajectory.steps if step.tool_calls)
        self.assertIsNotNone(tool_step.observation)
        assert tool_step.observation is not None
        self.assertIsNone(tool_step.observation.results[0].source_call_id)

    def test_model_response_does_not_attach_metrics_to_older_agent_step(self) -> None:
        events = parse_rara_jsonl(
            "\n".join(
                [
                    '{"type":"item.completed","item":{"id":"msg_1","type":"agent_message","text":"first"}}',
                    '{"type":"item.completed","item":{"id":"resp_1","type":"model_response","model":"mock","output_tokens":1,"finish_reason":"stop"}}',
                    '{"type":"item.completed","item":{"id":"msg_2","type":"agent_message","text":"second"}}',
                    '{"type":"item.completed","item":{"id":"resp_2","type":"model_response","model":"mock","output_tokens":2,"finish_reason":"stop"}}',
                    '{"type":"item.completed","item":{"id":"resp_3","type":"model_response","model":"mock","output_tokens":3,"finish_reason":"stop"}}',
                ]
            )
        )

        trajectory = convert_rara_events_to_trajectory(
            events,
            instruction="Check metrics.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        agent_steps = [step for step in trajectory.steps if step.source == "agent"]
        self.assertEqual(agent_steps[0].metrics.completion_tokens, 1)
        self.assertEqual(agent_steps[1].metrics.completion_tokens, 2)

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
            "apply_patch, replace, replace_lines, multi_edit, or write_file",
            uploaded_instruction,
        )
        self.assertIn("Do not use shell redirection, heredocs, sed, awk, perl", uploaded_instruction)
        self.assertIn("Use shell commands for process execution", uploaded_instruction)
        self.assertIn("Treat task constraints as validation requirements", uploaded_instruction)
        self.assertIn("substitutions must come from an allowed list", uploaded_instruction)
        self.assertIn("request escalated sandbox permissions", uploaded_instruction)
        self.assertIn("/app/solution.sparql", uploaded_instruction)
        self.assertIn("Do not finish with only an explanation", uploaded_instruction)

    def test_build_benchmark_instruction_preserves_task_text(self) -> None:
        instruction = "\nCreate /app/solution.sparql.\n"

        wrapped = build_benchmark_instruction(instruction, "/app")

        self.assertTrue(wrapped.endswith("Create /app/solution.sparql.\n"))

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
