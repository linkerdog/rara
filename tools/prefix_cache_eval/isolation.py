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
    command = [str(python), "-I", "-S", str(worker), str(workspace)]
    if sys.platform == "darwin":
        read_roots = [
            Path("/System/Library"),
            Path("/usr/lib"),
            runtime,
            python.parent,
            worker.parent,
            workspace,
        ]
        reads = " ".join(f"(subpath {json.dumps(str(p))})" for p in read_roots)
        profile = "\n".join(
            [
                "(version 1)",
                "(deny default)",
                '(import "/System/Library/Sandbox/Profiles/dyld-support.sb")',
                "(allow sysctl-read)",
                "(allow process-exec)",
                # Metadata permits resolving runtime symlinks, not reading sources.
                "(allow file-read-metadata)",
                f"(allow file-map-executable {reads})",
                f'(allow file-read* (literal "/") {reads})',
                '(allow file-read* (literal "/dev/random") (literal "/dev/urandom"))',
                '(allow file-read* file-write* (literal "/dev/null"))',
                f"(allow file-write* (subpath {json.dumps(str(workspace))}))",
                # Also excludes verifier symlinks and unusual runtime layouts.
                f"(deny file-read* (subpath {json.dumps(str(verifier))}))",
                # These pure function fixtures never need descendants or network.
                "(deny process-fork)",
            ]
        )
        return ["/usr/bin/sandbox-exec", "-p", profile, *command]
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
        return args
    # In particular, never run candidate code on Windows without a job/sandbox.
    raise OSError("no supported worker sandbox")


@contextmanager
def isolated_worker(workspace, source, output):
    with tempfile.TemporaryDirectory(prefix="cache-worker-") as directory:
        worker_path = Path(directory).resolve() / "worker.py"
        worker_path.write_bytes(Path(__file__).with_name("worker.py").read_bytes())
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
