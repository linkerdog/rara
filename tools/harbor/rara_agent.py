"""Harbor adapter for running RARA through ``rara exec``.

Load dynamically with:

    PYTHONPATH=$PWD/tools/harbor harbor run -d terminal-bench/terminal-bench-2 \
      --agent rara_agent:RaraAgent \
      --agent-kwarg binary_path=$PWD/target/release/rara
"""

from __future__ import annotations

import json
import os
import shlex
from pathlib import Path, PurePosixPath
from typing import Any

from harbor.agents.installed.base import BaseInstalledAgent, with_prompt_template
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext
from harbor.models.trial.paths import EnvironmentPaths


DEFAULT_REMOTE_BINARY = PurePosixPath("/installed-agent/rara")
DEFAULT_RARA_HOME = PurePosixPath("/logs/agent/rara-home")
DEFAULT_JSONL_PATH = EnvironmentPaths.agent_dir / "rara-exec.jsonl"
DEFAULT_INSTRUCTION_PATH = EnvironmentPaths.agent_dir / "instruction.txt"
DEFAULT_LAST_MESSAGE_PATH = EnvironmentPaths.agent_dir / "last-message.txt"


class RaraAgent(BaseInstalledAgent):
    """Run RARA as a Harbor installed agent using the headless exec surface."""

    SUPPORTS_ATIF = False
    SUPPORTS_WINDOWS = False

    def __init__(
        self,
        *args: Any,
        binary_path: str | None = None,
        remote_binary: str | None = None,
        cwd: str = ".",
        rara_home: str | None = None,
        **kwargs: Any,
    ) -> None:
        super().__init__(*args, **kwargs)
        self.binary_path = Path(
            binary_path or os.environ.get("RARA_HARBOR_BINARY", "target/release/rara")
        ).expanduser()
        self.remote_binary = PurePosixPath(remote_binary or DEFAULT_REMOTE_BINARY)
        self.cwd = cwd
        self.rara_home = PurePosixPath(rara_home or DEFAULT_RARA_HOME)

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

    @with_prompt_template
    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        instruction_path = self.logs_dir / "instruction.txt"
        instruction_path.parent.mkdir(parents=True, exist_ok=True)
        instruction_path.write_text(instruction)
        await environment.upload_file(
            instruction_path,
            DEFAULT_INSTRUCTION_PATH.as_posix(),
        )

        env = {**self.extra_env, "RARA_HOME": self.rara_home.as_posix()}
        command = self._build_exec_command()
        result = await environment.exec(command=command, cwd=self.cwd, env=env)

        stdout = result.stdout or ""
        events = parse_rara_jsonl(stdout)
        self._populate_context(context, events)
        if result.return_code != 0:
            raise self._classify_exec_error(command, result)

    def _build_exec_command(self) -> str:
        binary = shlex.quote(self.remote_binary.as_posix())
        jsonl_path = shlex.quote(DEFAULT_JSONL_PATH.as_posix())
        instruction_path = shlex.quote(DEFAULT_INSTRUCTION_PATH.as_posix())
        last_message_path = shlex.quote(DEFAULT_LAST_MESSAGE_PATH.as_posix())
        run_id = shlex.quote(self.context_id.hex if self.context_id else "harbor")
        task_id = shlex.quote(self.session_id or "harbor-task")
        return (
            f"mkdir -p {shlex.quote(EnvironmentPaths.agent_dir.as_posix())} "
            f"{shlex.quote(self.rara_home.as_posix())}\n"
            f"{binary} exec --json --run-id {run_id} --task-id {task_id} "
            f"--output-last-message {last_message_path} - "
            f"< {instruction_path} 2>&1 | tee {jsonl_path}"
        )

    @staticmethod
    def _populate_context(context: AgentContext, events: list[dict[str, Any]]) -> None:
        final_message: str | None = None
        failure: str | None = None
        input_tokens = 0
        output_tokens = 0
        event_counts: dict[str, int] = {}

        for event in events:
            event_type = event.get("type")
            if isinstance(event_type, str):
                event_counts[event_type] = event_counts.get(event_type, 0) + 1
            if event_type == "turn.completed":
                usage = event.get("usage") or {}
                input_tokens = int(usage.get("input_tokens") or input_tokens)
                output_tokens = int(usage.get("output_tokens") or output_tokens)
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
        }


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
