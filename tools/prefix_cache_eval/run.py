"""Create task inputs or grade an isolated workspace; never call a model."""

import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tempfile

from cases import CASES


def corpus_digest():
    root = Path(__file__).parent
    digest = hashlib.sha256()
    for name in ["cases.py", "grader.py", "run.py"]:
        digest.update(name.encode())
        digest.update((root / name).read_bytes())
    return digest.hexdigest()


def initialize(case_id, workspace):
    # Refuse reuse so a candidate cannot inherit a previous arm's repair.
    workspace.mkdir(parents=True, exist_ok=False)
    for name, content in CASES[case_id]["files"].items():
        (workspace / name).write_text(content)


def grade(case_id, phase, workspace, timeout=10):
    workspace = workspace.resolve()
    result = {"case_id": case_id, "phase": phase, "passed": False}
    root = Path(__file__).resolve().parent
    policy = CASES[case_id]["files"].get("POLICY.md")
    if policy is not None:
        try:
            if (workspace / "POLICY.md").read_text() != policy:
                return {**result, "reason": "protected_input_changed"}
        except OSError:
            return {**result, "reason": "protected_input_missing"}
    # Keep verifier code outside the model's workspace. The subprocess inherits
    # the surrounding task sandbox; -I isolates Python imports, not filesystem access.
    with tempfile.TemporaryFile() as output:
        try:
            completed = subprocess.run(
                [
                    sys.executable,
                    "-I",
                    str(root / "grader.py"),
                    str(case_id),
                    str(phase),
                    str(workspace),
                ],
                cwd=workspace,
                stdin=subprocess.DEVNULL,
                stdout=output,
                stderr=subprocess.DEVNULL,
                timeout=timeout,
                check=False,
            )
        except subprocess.TimeoutExpired:
            return {**result, "reason": "timeout"}
        except OSError:
            return {**result, "passed": None, "reason": "grader_unavailable"}
        output.seek(0)
        try:
            receipt = json.loads(output.read(4096))
        except (ValueError, UnicodeDecodeError):
            return {**result, "passed": None, "reason": "invalid_grader_receipt"}
        if (
            completed.returncode != 0
            or not isinstance(receipt, dict)
            or receipt != {**result, "passed": receipt.get("passed")}
            or not isinstance(receipt.get("passed"), bool)
        ):
            return {**result, "passed": None, "reason": "invalid_grader_receipt"}
    return receipt


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
