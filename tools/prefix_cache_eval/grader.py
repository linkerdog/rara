"""Verify worker observations without importing or executing candidate modules."""

import copy
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile


def window_checks():
    for length in range(9):
        items = list(range(length))
        for offset in range(12):
            for limit in range(12):
                arguments = [items, offset, limit]
                expected = [
                    item
                    for index, item in enumerate(items)
                    if offset <= index < offset + limit
                ]
                yield {"function": "window", "arguments": arguments}, {
                    "value": expected,
                    "arguments": copy.deepcopy(arguments),
                    "aliases_arguments": [False, False, False],
                    "error": None,
                }
    for items in [[], [1, 2]]:
        for offset, limit in [(-1, 2), (1, -1), (-1, -1)]:
            yield {"function": "window", "arguments": [items, offset, limit]}, {
                "error": {"value_error": True}
            }
    yield {"function": "window", "arguments": [[1, 2], 0, 10**30]}, {
        "value": [1, 2],
        "error": None,
    }


def record_checks():
    for text, expected in [
        ("", []),
        (" \n\t\r\n", []),
        (
            '{"id":1}\n\n{"nested":{"ok":true,"value":null}}\r\n',
            [{"id": 1}, {"nested": {"ok": True, "value": None}}],
        ),
        ('{"id":1}\n{"id":1}', [{"id": 1}, {"id": 1}]),
    ]:
        yield {"function": "parse_objects", "arguments": [text]}, {
            "value": expected,
            "error": None,
        }
    for text, line_number, decode_error in [
        ('{\n{"id":1}', 1, True),
        ('\n{"id":1}\n \n{\n', 4, True),
        ('{"id":1}\n{\n[', 2, True),
        ("\n[]\n", 2, False),
        ('{"id":1}\nnull\n', 2, False),
        ('"record"', 1, False),
        ("42", 1, False),
    ]:
        error = {
            "line_number": line_number,
            "value_error": True,
            "record_error": True,
        }
        if decode_error:
            error["json_decode_cause"] = True
        yield {"function": "parse_objects", "arguments": [text]}, {"error": error}


def identity_checks(phase):
    inputs = [
        [],
        [{"id": "B", "value": 1}, {"id": "A", "value": 2}],
        [{"id": "X", "value": 1}, {"id": "x", "value": 2}, {"id": "X", "value": 3}],
        [{"id": "", "value": 1}, {"id": "", "value": 2}],
        [{"id": "\u00df", "value": 1}, {"id": "ss", "value": 2}],
        [{"id": "A", "value": {"nested": [1, 2]}}, {"id": "A", "value": 3}],
    ]
    calls = [("stable_unique", [records]) for records in inputs]
    if phase == 2:
        calls.extend(
            ("merge_unique", [existing, incoming])
            for existing in inputs
            for incoming in inputs
        )
    for function, arguments in calls:
        combined = [record for records in arguments for record in records]
        origins = [
            i
            for i, record in enumerate(combined)
            if all(old["id"] != record["id"] for old in combined[:i])
        ]
        yield {"function": function, "arguments": arguments}, {
            "value": [combined[i] for i in origins],
            "item_origins": origins,
            "arguments": copy.deepcopy(arguments),
            "aliases_arguments": [False] * len(arguments),
            "error": None,
        }


def grade_candidate(case_id, phase, workspace, timeout):
    if case_id == 1:
        checks = list(window_checks())
    elif case_id == 2:
        checks = list(record_checks())
    elif case_id == 3:
        checks = list(identity_checks(phase))
    else:
        raise ValueError("unknown case")
    # Only calls cross into the worker; expected results remain in this process.
    with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as output:
        source.write(json.dumps([request for request, _ in checks]).encode())
        source.seek(0)
        try:
            with subprocess.Popen(
                [
                    sys.executable,
                    "-I",
                    str(Path(__file__).with_name("worker.py")),
                    str(workspace),
                ],
                cwd=workspace,
                stdin=source,
                stdout=output,
                stderr=subprocess.DEVNULL,
                start_new_session=os.name == "posix",
            ) as worker:
                try:
                    worker.wait(timeout=timeout)
                except subprocess.TimeoutExpired:
                    if os.name == "posix":
                        try:
                            os.killpg(worker.pid, signal.SIGKILL)
                        except ProcessLookupError:
                            pass
                    else:
                        worker.kill()
                    worker.wait()
                    return {"passed": False, "reason": "timeout"}
        except OSError:
            return {"passed": None, "reason": "grader_unavailable"}
        output.seek(0)
        try:
            receipt = json.loads(output.read(2_000_001))
            if worker.returncode != 0 or output.tell() > 2_000_000:
                raise ValueError("invalid worker output")
            if receipt == {"candidate_error": True}:
                return {"passed": False}
            if not isinstance(receipt, dict) or set(receipt) != {"observations"}:
                raise ValueError("invalid worker receipt")
            observations = receipt["observations"]
            if not isinstance(observations, list) or len(observations) != len(checks):
                raise ValueError("missing worker observations")
            if not all(isinstance(observation, dict) for observation in observations):
                raise ValueError("invalid worker observation")
        except (ValueError, UnicodeDecodeError):
            return {"passed": None, "reason": "invalid_grader_receipt"}
    for observation, (_, expected) in zip(observations, checks):
        for key, value in expected.items():
            actual = observation.get(key)
            if key == "error" and isinstance(value, dict):
                if not isinstance(actual, dict) or any(
                    actual.get(k) != v for k, v in value.items()
                ):
                    return {"passed": False}
            elif actual != value:
                return {"passed": False}
    return {"passed": True}
