"""Create task inputs or grade an isolated workspace; never call a model."""

import argparse
import hashlib
import json
from pathlib import Path
import sys

from cases import CASES
from grader import grade_candidate


def corpus_digest():
    root = Path(__file__).parent
    digest = hashlib.sha256()
    for name in ["cases.py", "grader.py", "worker.py", "run.py"]:
        digest.update(name.encode())
        digest.update((root / name).read_bytes())
    return digest.hexdigest()


def initialize(case_id, workspace):
    # Refuse reuse so a candidate cannot inherit a previous arm's repair.
    workspace.mkdir(parents=True, exist_ok=False)
    for name, content in CASES[case_id]["files"].items():
        (workspace / name).write_text(content)


def protected_input_reason(case_id, workspace):
    policy = CASES[case_id]["files"].get("POLICY.md")
    if policy is not None:
        try:
            if (workspace / "POLICY.md").read_text() != policy:
                return "protected_input_changed"
        except (OSError, UnicodeError):
            return "protected_input_missing"
    return None


def grade(case_id, phase, workspace, timeout=10):
    workspace = workspace.resolve()
    result = {"case_id": case_id, "phase": phase, "passed": False}
    reason = protected_input_reason(case_id, workspace)
    if reason is not None:
        return {**result, "reason": reason}
    receipt = grade_candidate(case_id, phase, workspace, timeout)
    # Revalidate even when the worker failed, timed out, or returned no receipt.
    reason = protected_input_reason(case_id, workspace)
    if reason is not None:
        return {**result, "reason": reason}
    return {**result, **receipt}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=["show", "init", "grade", "export"])
    parser.add_argument("--case", type=int, choices=CASES)
    parser.add_argument("--workspace", type=Path)
    parser.add_argument("--phase", type=int, choices=[1, 2], default=1)
    args = parser.parse_args()
    if args.action == "export":
        selected = CASES if args.case is None else {args.case: CASES[args.case]}
        print(
            json.dumps(
                {
                    "corpus_sha256": corpus_digest(),
                    "python_version": sys.version.split()[0],
                    "cases": [
                        {"case_id": case_id, **case}
                        for case_id, case in selected.items()
                    ],
                }
            )
        )
        return 0
    if args.case is None:
        parser.error("--case is required for show, init, and grade")
    if args.phase == 2 and args.case != 3:
        parser.error("only case 3 has a second grading phase")
    if args.action != "show" and args.workspace is None:
        parser.error("--workspace is required for init and grade")
    receipt = {"case_id": args.case, "corpus_sha256": corpus_digest()}
    if args.action == "show":
        receipt["turns"] = CASES[args.case]["turns"]
    elif args.action == "init":
        initialize(args.case, args.workspace)
    else:
        receipt.update(grade(args.case, args.phase, args.workspace))
    print(json.dumps(receipt))
    return 0 if receipt.get("passed", True) else 1


if __name__ == "__main__":
    sys.exit(main())
