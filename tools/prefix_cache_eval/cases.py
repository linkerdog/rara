"""Deterministic task inputs; graders and reference repairs stay outside workspaces."""

CASES = {
    1: {
        "files": {
            "task.py": """def window(items, offset, limit):
    return items[offset:offset + limit - 1]
""",
        },
        "turns": [
            {
                "mode": "plan",
                "prompt": "Read task.py and explain the smallest repair for window(items, offset, limit). It must return a new list with up to limit items starting at the zero-based offset. Negative offset or limit must raise ValueError, including for empty input. Out-of-range offsets return an empty list. Inputs must remain unchanged. Do not edit yet.",
            },
            {
                "mode": "execute",
                "prompt": "Implement the reviewed contract in task.py and verify its boundary cases. Preserve the public function signature and use only the Python standard library.",
                "grade_phase": 1,
            },
            {
                "mode": "review",
                "prompt": "Review the resulting function against the original contract and report any remaining failure. Do not edit files.",
            },
        ],
    },
    2: {
        "files": {
            "task.py": """import json


class RecordError(ValueError):
    def __init__(self, line_number):
        self.line_number = line_number
        super().__init__(f"invalid record on line {line_number}")


def parse_objects(text):
    records = []
    for line_number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(value, dict):
            records.append(value)
    return records
""",
        },
        "turns": [
            {
                "mode": "plan",
                "prompt": "Read task.py and plan a repair for parse_objects. It parses newline-delimited JSON objects, skipping blank lines. It must fail on the first malformed JSON line or non-object record by raising RecordError with the original one-based physical line_number. A JSON decoding failure must be preserved as the exception cause. Valid records retain their order and values. Do not edit yet.",
            },
            {
                "mode": "execute",
                "prompt": "Repair task.py according to the reviewed contract. Preserve the public names and verify both valid input and diagnostic behavior using only the Python standard library.",
                "grade_phase": 1,
            },
            {
                "mode": "review",
                "prompt": "Review the error and success paths against the original contract. Report uncovered failures without editing files.",
            },
        ],
    },
    3: {
        "files": {
            "POLICY.md": """# Record Identity Contract

Record IDs are case-sensitive strings. Empty IDs and non-ASCII IDs are valid.
Keep the first record for each exact ID, in encounter order. Do not normalize,
sort, merge fields, or replace an earlier record. Return a new list containing
the original record objects; do not mutate either input list or any record.

For merging, existing records precede incoming records, including when existing
already contains duplicate IDs. Both helpers follow this same contract.
""",
            "task.py": """def stable_unique(records):
    by_id = {}
    for record in records:
        by_id[record["id"].casefold()] = record
    return list(by_id.values())


def merge_unique(existing, incoming):
    raise NotImplementedError
""",
        },
        "turns": [
            {
                "mode": "execute",
                "prompt": "Read POLICY.md and task.py. Fix stable_unique according to the policy and verify the invariants. Leave merge_unique for the next task. Do not change POLICY.md. Finish with a concise handoff recording the contract and its source.",
                "grade_phase": 1,
                "compaction_boundary_after": True,
            },
            {
                "mode": "execute",
                "prompt": "Implement merge_unique under the identity policy reviewed in the previous task. Keep stable_unique correct, preserve public signatures, and verify the combined behavior. Do not change POLICY.md. Use only the Python standard library.",
                "grade_phase": 2,
            },
            {
                "mode": "review",
                "prompt": "Review both helpers against the policy and the previous handoff. Report any remaining failures without editing files.",
            },
        ],
    },
}
