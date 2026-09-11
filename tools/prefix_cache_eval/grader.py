"""External contract checks. Execute inside the task's isolated environment."""

import contextlib
import copy
import json
import os
from pathlib import Path
import sys
import types


def check_window(candidate):
    for length in range(9):
        items = list(range(length))
        for offset in range(12):
            for limit in range(12):
                before = items.copy()
                expected = [
                    item
                    for index, item in enumerate(items)
                    if offset <= index < offset + limit
                ]
                actual = candidate.window(items, offset, limit)
                assert actual == expected and actual is not items
                assert items == before
    for items in [[], [1, 2]]:
        for offset, limit in [(-1, 2), (1, -1), (-1, -1)]:
            try:
                candidate.window(items, offset, limit)
            except ValueError:
                pass
            else:
                raise AssertionError("negative bound accepted")
    assert candidate.window([1, 2], 0, 10**30) == [1, 2]


def check_records(candidate):
    for text, expected in [
        ("", []),
        (" \n\t\r\n", []),
        (
            '{"id":1}\n\n{"nested":{"ok":true,"value":null}}\r\n',
            [{"id": 1}, {"nested": {"ok": True, "value": None}}],
        ),
        ('{"id":1}\n{"id":1}', [{"id": 1}, {"id": 1}]),
    ]:
        assert candidate.parse_objects(text) == expected
    for text, line_number, decode_error in [
        ('{\n{"id":1}', 1, True),
        ('\n{"id":1}\n \n{\n', 4, True),
        ('{"id":1}\n{\n[', 2, True),
        ("\n[]\n", 2, False),
        ('{"id":1}\nnull\n', 2, False),
        ('"record"', 1, False),
        ("42", 1, False),
    ]:
        try:
            candidate.parse_objects(text)
        except candidate.RecordError as error:
            assert error.line_number == line_number
            assert isinstance(error, ValueError)
            if decode_error:
                assert isinstance(error.__cause__, json.JSONDecodeError)
        else:
            raise AssertionError("invalid record accepted")


def check_identity(candidate, phase):
    inputs = [
        [],
        [{"id": "B", "value": 1}, {"id": "A", "value": 2}],
        [{"id": "X", "value": 1}, {"id": "x", "value": 2}, {"id": "X", "value": 3}],
        [{"id": "", "value": 1}, {"id": "", "value": 2}],
        [{"id": "\u00df", "value": 1}, {"id": "ss", "value": 2}],
        [{"id": "A", "value": {"nested": [1, 2]}}, {"id": "A", "value": 3}],
    ]
    for records in inputs:
        before = copy.deepcopy(records)
        expected = [
            record
            for i, record in enumerate(records)
            if all(old["id"] != record["id"] for old in records[:i])
        ]
        result = candidate.stable_unique(records)
        assert result == expected and result is not records
        assert all(actual is original for actual, original in zip(result, expected))
        assert records == before
    if phase == 1:
        return
    for existing in inputs:
        for incoming in inputs:
            before = copy.deepcopy((existing, incoming))
            combined = existing + incoming
            expected = [
                record
                for i, record in enumerate(combined)
                if all(old["id"] != record["id"] for old in combined[:i])
            ]
            result = candidate.merge_unique(existing, incoming)
            assert (
                result == expected and result is not existing and result is not incoming
            )
            assert all(actual is original for actual, original in zip(result, expected))
            assert (existing, incoming) == before


def grade(case_id, phase, workspace):
    candidate = types.ModuleType("candidate")
    candidate.__file__ = str(workspace / "task.py")
    sys.modules[candidate.__name__] = candidate
    try:
        with open(os.devnull, "w") as sink, contextlib.redirect_stdout(
            sink
        ), contextlib.redirect_stderr(sink):
            source = (workspace / "task.py").read_text()
            exec(compile(source, candidate.__file__, "exec"), candidate.__dict__)
            if case_id == 1:
                check_window(candidate)
            elif case_id == 2:
                check_records(candidate)
            elif case_id == 3:
                check_identity(candidate, phase)
            else:
                raise ValueError("unknown case")
    except BaseException:
        # Candidate source and exception messages do not belong in cost artifacts.
        return False
    return True


if __name__ == "__main__":
    case_id, phase = int(sys.argv[1]), int(sys.argv[2])
    passed = grade(case_id, phase, Path(sys.argv[3]))
    print(json.dumps({"case_id": case_id, "phase": phase, "passed": passed}))
