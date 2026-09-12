"""OS-enforced execution boundary for the observation-only Python worker."""

from contextlib import contextmanager
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile


def sandbox_command(worker, workspace):
    python = Path(sys.executable).resolve()
    runtime = Path(sys.base_prefix).resolve()
    verifier = Path(__file__).resolve().parent
    # A runtime or fixture root containing the verifier would expose the oracle.
    for root in [runtime, workspace, worker.parent]:
        if root == verifier or root in verifier.parents:
            raise OSError("sandbox roots overlap the verifier")
    command = [
        str(python),
        "-I",
        "-S",
        str(worker),
        str(workspace),
        str(worker.with_name("task.py")),
    ]
    if sys.platform == "linux":
        # Start from an empty filesystem; never bind the repository or host home.
        roots = [Path(p) for p in ["/usr", "/lib", "/lib64", "/bin"]]
        roots.extend([runtime, python.parent])
        args = [
            "/usr/bin/bwrap",
            "--unshare-all",
            "--die-with-parent",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--tmpfs",
            "/",
            "--dev",
            "/dev",
            "--proc",
            "/proc",
        ]
        mounted = []
        for root in roots:
            if not root.exists() or any(
                root == p or p in root.parents for p in mounted
            ):
                continue
            if root == verifier or root in verifier.parents:
                raise OSError("runtime mount exposes the verifier")
            args.extend(["--ro-bind", str(root), str(root)])
            mounted.append(root)
        args.extend(["--ro-bind", str(worker.parent), str(worker.parent)])
        args.extend(["--bind", str(workspace), str(workspace)])
        args.extend(["--chdir", str(workspace), "--", *command])
        # prlimit lowers hard limits before Bubblewrap or Python can run.
        return [
            "/usr/bin/prlimit",
            "--as=268435456:268435456",
            "--cpu=2:3",
            "--fsize=2000000:2000000",
            "--core=0:0",
            "--",
            *args,
        ]
    # Seatbelt does not provide a hard memory ceiling. Other hosts require
    # a Linux VM/container until an equivalent resource boundary is available.
    raise OSError("no supported worker sandbox")


@contextmanager
def isolated_worker(workspace, source, output, *, candidate_source=None):
    with tempfile.TemporaryDirectory(prefix="cache-worker-") as directory:
        worker_path = Path(directory).resolve() / "worker.py"
        worker_path.write_bytes(Path(__file__).with_name("worker.py").read_bytes())
        if candidate_source is None:
            candidate_source = (workspace / "task.py").read_text()
        worker_path.with_name("task.py").write_text(candidate_source)
        worker = subprocess.Popen(
            sandbox_command(worker_path, workspace),
            cwd=workspace,
            stdin=source,
            stdout=output,
            stderr=subprocess.DEVNULL,
            env={},
            start_new_session=True,
        )
        try:
            yield worker
        finally:
            # Normal exit, malformed receipts, exceptions, and timeouts all
            # drain the process group before the caller revalidates the policy.
            try:
                os.killpg(worker.pid, signal.SIGKILL)
            except ProcessLookupError:
                pass
            worker.wait()


def preflight():
    """Establish the same execution boundary before any paid model call."""
    with tempfile.TemporaryDirectory(prefix="cache-preflight-") as directory:
        workspace = Path(directory).resolve()
        (workspace / "task.py").write_text("def probe():\n    return 'ready'\n")
        with tempfile.TemporaryFile() as source, tempfile.TemporaryFile() as output:
            source.write(b'[{"function":"probe","arguments":[]}]')
            source.seek(0)
            try:
                with isolated_worker(workspace, source, output) as worker:
                    worker.wait(timeout=5)
            except (OSError, subprocess.TimeoutExpired):
                return False
            output.seek(0)
            try:
                receipt = json.loads(output.read(100_000))
                return (
                    worker.returncode == 0
                    and receipt["observations"][0]["value"] == "ready"
                )
            except (ValueError, KeyError, IndexError, TypeError):
                return False
