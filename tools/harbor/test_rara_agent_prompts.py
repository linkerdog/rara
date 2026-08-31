from __future__ import annotations

import unittest

from rara_agent_prompts import (
    build_benchmark_instruction,
    build_verification_instruction,
    last_completed_message,
    parse_optional_bool,
)


class RaraAgentPromptTests(unittest.TestCase):
    def test_parse_optional_bool_rejects_invalid_agent_kwarg(self) -> None:
        self.assertTrue(parse_optional_bool("yes", name="thinking"))
        self.assertFalse(parse_optional_bool("off", name="thinking"))
        self.assertIsNone(parse_optional_bool(None, name="thinking"))
        with self.assertRaisesRegex(ValueError, "thinking must be a boolean"):
            parse_optional_bool("sometimes", name="thinking")

    def test_build_benchmark_instruction_preserves_task_text(self) -> None:
        instruction = "\nCreate /app/solution.sparql.\n"

        wrapped = build_benchmark_instruction(instruction, "/app")

        task_text = "Create /app/solution.sparql."
        self.assertIn(task_text, wrapped)
        self.assertLess(wrapped.index(task_text), wrapped.index("Completion gate:"))
        self.assertTrue(wrapped.endswith("report it as unverified.\n"))

    def test_build_verification_instruction_is_task_derived_and_repair_oriented(self) -> None:
        instruction = build_verification_instruction(
            "Create /app/output.txt.",
            "/app",
            implementation_summary="Created the requested file and checked its format.",
        )

        self.assertIn("Work only in the benchmark workspace: /app", instruction)
        self.assertIn("Do not search for or depend on benchmark verifier code", instruction)
        self.assertIn("make the smallest robust repair", instruction)
        self.assertIn("Original task:\nCreate /app/output.txt.", instruction)
        self.assertIn(
            "Reported implementation and validation evidence:\nCreated the requested file",
            instruction,
        )
        self.assertLess(
            instruction.index("Reported implementation and validation evidence:"),
            instruction.index("Evidence-delta review protocol:"),
        )
        self.assertTrue(instruction.endswith("instead of claiming completion.\n"))

    def test_last_completed_message_returns_latest_non_empty_summary(self) -> None:
        events = [
            {"type": "turn.completed", "final_message": "implemented"},
            {"type": "turn.failed", "error": {"message": "retry"}},
            {"type": "turn.completed", "final_message": "verified"},
        ]

        self.assertEqual(last_completed_message(events), "verified")
        self.assertIsNone(last_completed_message([{"type": "turn.started"}]))


if __name__ == "__main__":
    unittest.main()
