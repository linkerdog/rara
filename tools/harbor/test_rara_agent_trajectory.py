from __future__ import annotations

import unittest

from rara_agent import convert_rara_events_to_trajectory, parse_rara_jsonl


class RaraAgentTrajectoryTests(unittest.TestCase):
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

    def test_convert_raw_combined_jsonl_infers_verification_phase(self) -> None:
        trajectory = convert_rara_events_to_trajectory(
            [
                {
                    "type": "thread.started",
                    "metadata": {"session_id": "implementation-session"},
                },
                {
                    "type": "thread.started",
                    "metadata": {"session_id": "verification-session"},
                },
            ],
            instruction="Implement /app/tool.py.",
        )

        self.assertIsNotNone(trajectory)
        assert trajectory is not None
        self.assertEqual(
            trajectory.final_metrics.extra["rara_sessions"],
            [
                {"phase": "implementation", "session_id": "implementation-session"},
                {"phase": "verification", "session_id": "verification-session"},
            ],
        )

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


if __name__ == "__main__":
    unittest.main()
