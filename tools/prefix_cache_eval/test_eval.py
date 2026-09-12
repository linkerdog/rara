"""Calibrate the graders with known repairs and deliberately wrong implementations."""

import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch

from cases import CASES
from run import corpus_digest, grade, initialize
from isolation import preflight

REPAIRS = {
    1: """def window(items, offset, limit):
    if offset < 0 or limit < 0:
        raise ValueError("negative bound")
    return list(items[offset:offset + limit])
""",
    2: CASES[2]["files"]["task.py"].split("def parse_objects")[0]
    + """def parse_objects(text):
    records = []
    for line_number, line in enumerate(text.splitlines(), 1):
        if not line.strip():
            continue
        try:
            value = json.loads(line)
        except json.JSONDecodeError as error:
            raise RecordError(line_number) from error
        if not isinstance(value, dict):
            raise RecordError(line_number)
        records.append(value)
    return records
""",
    3: """def stable_unique(records):
    seen = set()
    result = []
    for record in records:
        key = record["id"]
        if key not in seen:
            seen.add(key)
            result.append(record)
    return result


def merge_unique(existing, incoming):
    return stable_unique(existing + incoming)
""",
}


class GraderCalibration(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def workspace(self, case_id, source=None):
        workspace = self.root / str(case_id)
        initialize(case_id, workspace)
        if source is not None:
            (workspace / "task.py").write_text(source)
        return workspace

    def test_every_starter_fails_and_reference_repair_passes(self):
        for case_id in CASES:
            with self.subTest(case_id=case_id):
                workspace = self.workspace(case_id)
                phase = 2 if case_id == 3 else 1
                self.assertFalse(grade(case_id, phase, workspace)["passed"])
                (workspace / "task.py").write_text(REPAIRS[case_id])
                self.assertTrue(grade(case_id, phase, workspace)["passed"])

    def test_negative_bounds_are_not_hidden_by_a_correct_slice(self):
        source = "def window(items, offset, limit):\n    return list(items[offset:offset + limit])\n"
        self.assertFalse(grade(1, 1, self.workspace(1, source))["passed"])

    def test_physical_line_numbers_survive_blank_lines(self):
        source = REPAIRS[2].replace(
            "text.splitlines()", "filter(str.strip, text.splitlines())"
        )
        self.assertFalse(grade(2, 1, self.workspace(2, source))["passed"])

    def test_decoding_failure_must_keep_its_cause(self):
        source = REPAIRS[2].replace("from error", "from None")
        self.assertFalse(grade(2, 1, self.workspace(2, source))["passed"])

    def test_casefolding_changes_record_identity(self):
        source = REPAIRS[3].replace(
            'key = record["id"]', 'key = record["id"].casefold()'
        )
        self.assertFalse(grade(3, 1, self.workspace(3, source))["passed"])

    def test_reversing_merge_order_loses_the_previous_task_contract(self):
        source = REPAIRS[3].replace("existing + incoming", "incoming + existing")
        workspace = self.workspace(3, source)
        self.assertTrue(grade(3, 1, workspace)["passed"])
        self.assertFalse(grade(3, 2, workspace)["passed"])

    def test_copying_record_objects_is_not_contract_equivalent(self):
        source = REPAIRS[3].replace(
            "result.append(record)", "result.append(dict(record))"
        )
        self.assertFalse(grade(3, 1, self.workspace(3, source))["passed"])

    def test_policy_edits_cannot_make_a_task_pass(self):
        workspace = self.workspace(3, REPAIRS[3])
        (workspace / "POLICY.md").write_text("revised policy")
        receipt = grade(3, 2, workspace)
        self.assertFalse(receipt["passed"])
        self.assertEqual(receipt["reason"], "protected_input_changed")

    def test_candidate_cannot_replace_verifier_functions(self):
        for case_id in CASES:
            with self.subTest(case_id=case_id):
                source = CASES[case_id]["files"]["task.py"] + "\nimport __main__\n"
                source += "__main__.check_window = lambda *args: None\n"
                source += "__main__.check_records = lambda *args: None\n"
                source += "__main__.check_identity = lambda *args: None\n"
                source += "__main__.grade = lambda *args: True\n"
                self.assertIs(
                    grade(
                        case_id,
                        2 if case_id == 3 else 1,
                        self.workspace(case_id, source),
                    )["passed"],
                    False,
                )

    def test_policy_changes_during_candidate_execution_fail(self):
        for mutation in ["write_text('changed')", "unlink()"]:
            with self.subTest(
                mutation=mutation
            ), tempfile.TemporaryDirectory() as directory:
                workspace = Path(directory) / "workspace"
                initialize(3, workspace)
                source = (
                    "from pathlib import Path\nPath('POLICY.md')."
                    + mutation
                    + "\n"
                    + REPAIRS[3]
                )
                (workspace / "task.py").write_text(source)
                self.assertIs(grade(3, 2, workspace)["passed"], False)

    def test_workspace_reuse_cannot_copy_repairs_between_arms(self):
        workspace = self.workspace(1, REPAIRS[1])
        with self.assertRaises(FileExistsError):
            initialize(1, workspace)
        self.assertEqual((workspace / "task.py").read_text(), REPAIRS[1])

    def test_verifier_is_unreadable_by_absolute_path_argv_or_symlink(self):
        verifier = Path(__file__).resolve().parent
        workspace = self.workspace(1)
        (workspace / "oracle.py").symlink_to(verifier / "grader.py")
        source = f"""from pathlib import Path
import sys
paths = [Path(sys.argv[0]).with_name('grader.py'), Path('oracle.py')]
paths.extend(Path({str(verifier)!r}) / name for name in ['grader.py', 'cases.py', 'test_eval.py'])
for path in paths:
    try:
        path.read_bytes()
    except OSError:
        continue
    raise RuntimeError('verifier source was exposed')
""" + REPAIRS[1]
        (workspace / "task.py").write_text(source)
        self.assertIs(grade(1, 1, workspace)["passed"], True)

    def test_worker_has_no_inherited_host_credentials(self):
        source = "import os\nassert 'CACHE_TRIAL_FAKE_SECRET' not in os.environ\n"
        with patch.dict(os.environ, {"CACHE_TRIAL_FAKE_SECRET": "fixture-secret"}):
            self.assertIs(
                grade(1, 1, self.workspace(1, source + REPAIRS[1]))["passed"], True
            )

    def test_worker_cannot_connect_to_a_host_service(self):
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            listener.listen()
            port = listener.getsockname()[1]
            source = f"""import socket
try:
    socket.create_connection(('127.0.0.1', {port}), timeout=0.1)
except OSError:
    pass
else:
    raise RuntimeError('host network was exposed')
""" + REPAIRS[1]
            self.assertIs(grade(1, 1, self.workspace(1, source))["passed"], True)

    def test_delayed_descendants_cannot_outlive_any_worker_exit(self):
        for ending, expected in [
            (REPAIRS[3], True),
            ("os._exit(0)\n", None),
            ("raise RuntimeError('failed')\n", False),
            ("while True:\n    pass\n", False),
        ]:
            with self.subTest(expected=expected), tempfile.TemporaryDirectory() as d:
                workspace = Path(d) / "workspace"
                initialize(3, workspace)
                child = "import time; from pathlib import Path; time.sleep(0.6); Path('POLICY.md').write_text('late mutation')"
                source = f"""import os
import subprocess
import sys
from pathlib import Path
try:
    subprocess.Popen([sys.executable, '-I', '-S', '-c', {child!r}], start_new_session=True)
    Path('spawned').touch()
except OSError:
    pass
""" + ending
                (workspace / "task.py").write_text(source)
                receipt = grade(3, 2, workspace, timeout=0.4)
                self.assertIs(receipt["passed"], expected)
                if sys.platform == "linux":
                    self.assertTrue((workspace / "spawned").exists())
                time.sleep(0.7)
                self.assertEqual(
                    (workspace / "POLICY.md").read_text(),
                    CASES[3]["files"]["POLICY.md"],
                )

    def test_unsupported_platform_never_executes_candidate_code(self):
        workspace = self.workspace(1, "from pathlib import Path\nPath('ran').touch()\n")
        with patch("isolation.sys.platform", "win32"):
            receipt = grade(1, 1, workspace)
            self.assertIsNone(receipt["passed"])
            self.assertEqual(receipt["reason"], "grader_unavailable")
            self.assertFalse(preflight())
        self.assertFalse((workspace / "ran").exists())

    def test_sandbox_start_failure_never_falls_back_to_bare_python(self):
        workspace = self.workspace(1, "from pathlib import Path\nPath('ran').touch()\n")
        with patch(
            "isolation.sandbox_command", return_value=["/missing/cache-sandbox"]
        ):
            self.assertIsNone(grade(1, 1, workspace)["passed"])
            self.assertFalse(preflight())
        self.assertFalse((workspace / "ran").exists())

    def test_preflight_proves_the_worker_can_execute_before_paid_calls(self):
        self.assertTrue(preflight())

    def test_policy_is_rechecked_after_timeout_or_missing_receipt(self):
        for ending in ["while True:\n    pass\n", "import os\nos._exit(0)\n"]:
            with self.subTest(
                ending=ending
            ), tempfile.TemporaryDirectory() as directory:
                workspace = Path(directory) / "workspace"
                initialize(3, workspace)
                (workspace / "task.py").write_text(
                    "from pathlib import Path\nPath('POLICY.md').write_text('changed')\n"
                    + ending
                )
                receipt = grade(3, 2, workspace, timeout=0.2)
                self.assertIs(receipt["passed"], False)
                self.assertEqual(receipt["reason"], "protected_input_changed")

    def test_worker_receipts_cannot_bypass_observation_validation(self):
        source = """import json
import os
os.write(1, json.dumps({"case_id": 1, "phase": 1, "passed": True}).encode())
os._exit(0)
"""
        receipt = grade(1, 1, self.workspace(1, source))
        self.assertIsNone(receipt["passed"])
        self.assertEqual(receipt["reason"], "invalid_grader_receipt")

    def test_timeout_and_candidate_errors_are_failed_grades(self):
        workspace = self.workspace(1, "while True:\n    pass\n")
        self.assertEqual(grade(1, 1, workspace, timeout=0.2)["reason"], "timeout")
        (workspace / "task.py").write_text(
            'print("private-sentinel")\nraise RuntimeError("private-sentinel")'
        )
        receipt = grade(1, 1, workspace)
        self.assertFalse(receipt["passed"])
        self.assertNotIn("private-sentinel", json.dumps(receipt))

    def test_cli_exposes_phases_and_emits_content_free_grades(self):
        script = Path(__file__).with_name("run.py")
        show = subprocess.run(
            [sys.executable, str(script), "show", "--case", "3"],
            check=True,
            capture_output=True,
            text=True,
        )
        task = json.loads(show.stdout)
        self.assertTrue(task["turns"][0]["compaction_boundary_after"])
        self.assertEqual(task["corpus_sha256"], corpus_digest())
        workspace = self.workspace(3, REPAIRS[3])
        completed = subprocess.run(
            [
                sys.executable,
                str(script),
                "grade",
                "--case",
                "3",
                "--phase",
                "2",
                "--workspace",
                str(workspace),
            ],
            check=True,
            capture_output=True,
            text=True,
        )
        receipt = json.loads(completed.stdout)
        self.assertTrue(receipt["passed"])
        self.assertEqual(receipt["corpus_sha256"], corpus_digest())
        self.assertNotIn(str(workspace), completed.stdout)

    def test_missing_grader_receipt_is_unknown_instead_of_a_quality_result(self):
        workspace = self.workspace(1, "import os\nos._exit(0)\n")
        receipt = grade(1, 1, workspace)
        self.assertIsNone(receipt["passed"])
        self.assertEqual(receipt["reason"], "invalid_grader_receipt")

    def test_export_binds_driver_inputs_to_the_same_corpus_and_python(self):
        completed = subprocess.run(
            [sys.executable, str(Path(__file__).with_name("run.py")), "export"],
            check=True,
            capture_output=True,
            text=True,
        )
        exported = json.loads(completed.stdout)
        self.assertEqual(exported["corpus_sha256"], corpus_digest())
        self.assertEqual(exported["python_version"], sys.version.split()[0])
        self.assertEqual(
            exported["cases"],
            [{"case_id": case_id, **case} for case_id, case in CASES.items()],
        )


if __name__ == "__main__":
    unittest.main()
