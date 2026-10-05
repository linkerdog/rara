#!/usr/bin/env python3
"""Exercise the actual app-server binary with isolated state and a local fake provider."""

import argparse
import json
import os
from pathlib import Path
import selectors
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer


class Provider(BaseHTTPRequestHandler):
    calls = 0
    tool_issued = False
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
            controlled = [tool["function"]["name"] for tool in request.get("tools", [])
                          if tool["function"]["name"].startswith("mcp_")
                          and len(tool["function"]["name"]) == 64]
            invoke = bool(controlled) and not type(self).tool_issued
            if invoke:
                type(self).tool_issued = True
        tool_call = {
            "id": "owned-smoke-call", "type": "function",
            "function": {"name": controlled[0] if controlled else "unused",
                         "arguments": json.dumps({"value": 9})},
        }
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
            if invoke:
                chunks[0]["choices"][0]["delta"] = {
                    "role": "assistant", "tool_calls": [{"index": 0, **tool_call}],
                }
                chunks[1]["choices"][0]["finish_reason"] = "tool_calls"
            body = (
                "".join("data: " + json.dumps(chunk) + "\n\n" for chunk in chunks)
                + "data: [DONE]\n\n"
            ).encode()
            content_type = "text/event-stream"
        else:
            message = {"role": "assistant", "content": "smoke done"}
            if invoke:
                message = {"role": "assistant", "content": None, "tool_calls": [tool_call]}
            body = json.dumps({"choices": [{
                "message": message, "finish_reason": "tool_calls" if invoke else "stop",
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
        self.workspace = workspace
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
        self.sessions = set()
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
            assert {"mcp_source.register", "mcp_source.unregister", "mcp_source.query"}.issubset(
                payload["request_methods"]
            )
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
        if frame["type"] == "event":
            assert frame["payload"]["runtime_id"] == self.runtime_id
            assert frame["payload"]["session_id"] in self.sessions
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
        self.sessions.add(result["session_id"])
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

    def wait_turn(self, turn_id):
        def finished(frame):
            if frame.get("type") != "event":
                return False
            event = frame["payload"]["event"]
            return (event.get("turn_id") == turn_id
                    and event["event"].get("type") == "session"
                    and event["event"].get("payload", {}).get("type") == "turn_finished")
        while not any(finished(frame) for frame in self.frames):
            self.receive()

    def shutdown(self):
        self.send({"type": "shutdown", "payload": {
            "runtime_id": self.runtime_id, "request_id": "shutdown"
        }})
        assert self.ack("shutdown")["status"] == "accepted"
        while self.receive()["type"] != "shutdown_complete":
            pass
        assert not self.process.stdin.closed
        assert self.process.wait(timeout=10) == 0, "shutdown must finish while stdin is open"
        assert not self.buffer and self.process.stdout.read() == b"", (
            "completion must be the final stdout frame"
        )


def controlled_source(child, session_id, directory):
    log = Path(directory) / "mcp-calls.jsonl"
    fixture = Path(__file__).parent / "fixtures" / "controlled_mcp_server.py"
    register = child.control("register", {"type": "mcp_source", "payload": {
        "type": "register", "payload": {
            "source_id": "smoke", "command": sys.executable,
            "args": ["-u", str(fixture.resolve()), str(log)],
            "env": {"SOURCE_SCOPE": "scoped-smoke"},
        }
    }}, session_id)
    child.send(register)
    accepted = child.ack("register")
    assert accepted["status"] == "accepted", accepted
    child.send(register)
    assert child.ack("register") == accepted
    child.send(child.control("controlled-prompt", {"type": "input", "payload": {
        "type": "submit_user_prompt", "payload": {"prompt": "Call the controlled tool once."}
    }}, session_id))
    child.wait_turn(child.ack("controlled-prompt")["turn_id"])
    requests = [json.loads(line) for line in log.read_text().splitlines()]
    assert len(requests) == 1 and requests[0]["params"]["arguments"] == {"value": 9}, requests
    results = [frame["payload"]["event"]["event"]["payload"]["payload"]
               for frame in child.frames if frame["type"] == "event"
               and frame["payload"]["event"]["event"]["type"] == "tool"
               and frame["payload"]["event"]["event"]["payload"]["type"] == "result"]
    result = next(item for item in results if item["call_id"] == "owned-smoke-call")
    assert result["is_error"] is False, result
    # Native tool results use the ordinary compact transcript format, not raw JSON.
    assert "scoped-smoke" in result["content"] and str(child.workspace) in result["content"]
    child.send(child.control("unregister", {"type": "mcp_source", "payload": {
        "type": "unregister", "payload": {"source_id": "smoke"}
    }}, session_id))
    assert child.ack("unregister")["status"] == "accepted"
    if sys.platform == "linux":
        assert not Path("/proc", log.with_suffix(".pid").read_text()).exists()
    child.shutdown()


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
        for scenario in ["normal", "controlled_source", "eof", "malformed", "truncated", "output_loss"]:
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
                        child.wait_turn(accepted["turn_id"])
                        assert Provider.calls == before + 1, "duplicate prompt executed again"
                        assert any("smoke done" in json.dumps(frame) for frame in child.frames)
                        child.shutdown()
                    elif scenario == "controlled_source":
                        controlled_source(child, session_id, directory)
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
