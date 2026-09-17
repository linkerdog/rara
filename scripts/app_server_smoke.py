#!/usr/bin/env python3
"""Exercise the actual app-server binary with isolated state and a local fake provider."""

import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Provider(BaseHTTPRequestHandler):
    calls = 0
    lock = threading.Lock()

    def log_message(self, *_args):
        pass

    def do_POST(self):
        assert self.path.endswith("/chat/completions"), self.path
        size = int(self.headers["Content-Length"])
        assert size < 8 * 1024 * 1024
        request = json.loads(self.rfile.read(size))
        with self.lock:
            type(self).calls += 1
        if request.get("stream"):
            chunks = [
                {"choices": [{
                    "index": 0,
                    "delta": {"role": "assistant", "content": "smoke done"},
                    "finish_reason": None,
                }]},
                {
                    "choices": [{"index": 0, "delta": {}, "finish_reason": "stop"}],
                    "usage": {"prompt_tokens": 8, "completion_tokens": 2, "total_tokens": 10},
                },
            ]
            body = (
                "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks)
                + "data: [DONE]\n\n"
            ).encode()
            content_type = "text/event-stream"
        else:
            body = json.dumps({"choices": [{
                "message": {"role": "assistant", "content": "smoke done"},
                "finish_reason": "stop",
            }]}).encode()
            content_type = "application/json"
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        try:
            self.wfile.write(body)
        except BrokenPipeError:
            pass


class Child:
    def __init__(self, binary, base_url, directory):
        directory = Path(directory)
        workspace = directory / "workspace"
        workspace.mkdir()
        env = {
            "PATH": os.environ.get("PATH", "/usr/bin:/bin"),
            "RARA_HOME": str(directory / "state"),
            "SHELL": "/bin/sh",
            "TERM": "dumb",
        }
        self.process = subprocess.Popen(
            [str(binary), "app-server", "--protocol-version", "1", "--transport", "stdio-jsonl",
             "--provider", "deepseek", "--model", "fixture-model", "--api-key", "fixture-key",
             "--base-url", base_url, "--cwd", str(workspace),
             "--no-extension-discovery", "--no-memory-facilities"],
            cwd=workspace, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        )
        self.buffer = bytearray()
        self.frames = []
        self.diagnostics = bytearray()
        self.selector = selectors.DefaultSelector()
        self.selector.register(self.process.stdout, selectors.EVENT_READ)
        self.stderr_thread = threading.Thread(target=self._stderr, daemon=True)
        self.stderr_thread.start()
        try:
            hello = self.receive()
            assert hello["type"] == "handshake", hello
            payload = hello["payload"]
            assert payload["protocol_version"] == 1 and payload["transport"] == "stdio-jsonl"
            assert "server.shutdown" in payload["request_methods"]
            assert "session.resume" not in payload["request_methods"]
            assert payload["capabilities"]["approval_persistence"] is False
            # The native config normalizes the legacy provider name to its profile family.
            assert payload["provider"] == "openai-compatible"
            assert payload["model"] == "fixture-model"
            self.runtime_id = payload["runtime_id"]
        except BaseException:
            self.close()
            raise

    def _stderr(self):
        while chunk := self.process.stderr.read(4096):
            if len(self.diagnostics) < 65536:
                self.diagnostics.extend(chunk[:65536 - len(self.diagnostics)])

    def receive(self, seconds=15):
        deadline = time.monotonic() + seconds
        while b"\n" not in self.buffer:
            remaining = deadline - time.monotonic()
            assert remaining > 0 and self.selector.select(remaining), "output timeout"
            chunk = os.read(self.process.stdout.fileno(), 65536)
            assert chunk, f"unexpected output EOF: {self.diagnostics.decode(errors='replace')}"
            self.buffer.extend(chunk)
            assert len(self.buffer) <= 2 * 1048576, "unbounded output"
        line, _, remainder = self.buffer.partition(b"\n")
        self.buffer = bytearray(remainder)
        assert len(line) <= 1048576
        frame = json.loads(line)
        self.frames.append(frame)
        return frame

    def send(self, frame):
        self.process.stdin.write(json.dumps(frame).encode() + b"\n")
        self.process.stdin.flush()

    def control(self, request_id, request, session_id=None):
        return {"type": "control", "payload": {"runtime_id": self.runtime_id, "envelope": {
            "request_id": request_id,
            "provenance": {"controller": "app_server", "adapter": "stdio-jsonl",
                           "session_id": session_id,
                           "source_id": None, "trust": "untrusted", "authorship": "user_provided"},
            "request": request,
        }}}

    def ack(self, request_id):
        while True:
            frame = self.receive()
            if frame["type"] == "ack" and frame["payload"]["request_id"] == request_id:
                return frame["payload"]["result"]

    def create(self):
        request = self.control(
            "create", {"type": "session", "payload": {"type": "create_session"}}
        )
        self.send(request)
        result = self.ack("create")
        assert result["status"] == "accepted", result
        self.send(request)
        assert self.ack("create") == result
        return result["session_id"]

    def close(self):
        if self.process.poll() is None:
            self.process.kill()
            self.process.wait(timeout=5)
        self.selector.close()
        for stream in [self.process.stdin, self.process.stdout, self.process.stderr]:
            try:
                stream.close()
            except BrokenPipeError:
                pass
        self.stderr_thread.join(timeout=2)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    binary = parser.parse_args().binary.resolve(strict=True)
    server = ThreadingHTTPServer(("127.0.0.1", 0), Provider)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    base_url = f"http://127.0.0.1:{server.server_port}/v1"
    evidence = []
    try:
        for scenario in ["normal", "eof", "malformed", "truncated", "output_loss"]:
            with tempfile.TemporaryDirectory(prefix="app-server-smoke-") as directory:
                child = Child(binary, base_url, directory)
                try:
                    session_id = child.create()
                    if scenario == "normal":
                        before = Provider.calls
                        prompt = child.control("prompt", {"type": "input", "payload": {
                            "type": "submit_user_prompt", "payload": {"prompt": "Reply briefly."}
                        }}, session_id)
                        child.send(prompt)
                        accepted = child.ack("prompt")
                        assert accepted["status"] == "accepted" and accepted["turn_id"]
                        child.send(prompt)
                        assert child.ack("prompt") == accepted
                        def turn_finished(frame):
                            if frame.get("type") != "event":
                                return False
                            event = frame["payload"]["event"]["event"]
                            return (
                                event.get("type") == "session"
                                and event.get("payload", {}).get("type") == "turn_finished"
                            )

                        while not any(turn_finished(frame) for frame in child.frames):
                            child.receive()
                        assert Provider.calls == before + 1, "duplicate prompt executed again"
                        assert any("smoke done" in json.dumps(frame) for frame in child.frames)
                        child.send({"type": "shutdown", "payload": {
                            "runtime_id": child.runtime_id, "request_id": "shutdown"
                        }})
                        assert child.ack("shutdown")["status"] == "accepted"
                        while child.receive()["type"] != "shutdown_complete":
                            pass
                        assert not child.process.stdin.closed
                        assert child.process.wait(timeout=10) == 0, (
                            "shutdown must finish while stdin is open"
                        )
                        assert not child.buffer and child.process.stdout.read() == b"", (
                            "completion must be the final stdout frame"
                        )
                    else:
                        if scenario == "eof":
                            child.process.stdin.close()
                        elif scenario == "malformed":
                            child.process.stdin.write(b"not-json\n")
                            child.process.stdin.flush()
                        elif scenario == "truncated":
                            child.process.stdin.write(b'{"type":')
                            child.process.stdin.close()
                        else:
                            child.selector.unregister(child.process.stdout)
                            child.process.stdout.close()
                            child.send(child.control("state", {
                                "type": "session", "payload": {"type": "query_runtime_state"}
                            }, session_id))
                        assert child.process.wait(timeout=10) != 0, (
                            "transport loss is not semantic shutdown"
                        )
                        if scenario != "output_loss":
                            remaining = child.buffer + child.process.stdout.read()
                            for line in remaining.splitlines():
                                frame = json.loads(line)
                                assert frame["type"] != "shutdown_complete"
                                child.frames.append(frame)
                    evidence.append({
                        "scenario": scenario,
                        "exit_code": child.process.returncode,
                        "frames_observed": len(child.frames),
                    })
                finally:
                    child.close()
    finally:
        server.shutdown()
        server.server_close()
    print(json.dumps({
        "binary": str(binary),
        "provider": "isolated local fixture",
        "scenarios": evidence,
    }, indent=2))


if __name__ == "__main__":
    main()
