from __future__ import annotations

import unittest
from pathlib import Path
from uuid import UUID

from harbor.models.agent.context import AgentContext

from rara_agent import RaraAgent, parse_rara_jsonl


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

        self.assertEqual([event["type"] for event in events], ["thread.started", "turn.completed"])

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

    def test_build_exec_command_uses_context_and_session_ids(self) -> None:
        agent = RaraAgent(
            logs_dir=Path("/tmp/logs"),
            binary_path="/tmp/rara",
            remote_binary="/opt/rara",
            rara_home="/tmp/rara-home",
        )
        agent.context_id = UUID("00000000-0000-0000-0000-000000000123")
        agent.session_id = "trial-agent"

        command = agent._build_exec_command()

        self.assertIn("/opt/rara exec --json", command)
        self.assertIn("--run-id 00000000000000000000000000000123", command)
        self.assertIn("--task-id trial-agent", command)
        self.assertIn("--output-last-message /logs/agent/last-message.txt", command)
        self.assertIn("< /logs/agent/instruction.txt", command)
        self.assertIn("| tee /logs/agent/rara-exec.jsonl", command)


if __name__ == "__main__":
    unittest.main()
