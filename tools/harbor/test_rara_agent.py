from __future__ import annotations

import asyncio
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from uuid import UUID

from harbor.models.agent.context import AgentContext

from rara_agent import RaraAgent, build_benchmark_instruction, parse_rara_jsonl


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


class RaraAgentTests(unittest.TestCase):
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
