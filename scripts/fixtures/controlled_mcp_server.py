#!/usr/bin/env python3
"""Isolated MCP child for the executable app-server smoke test."""

import json
import os
from pathlib import Path
import sys


log = Path(sys.argv[1])
log.with_suffix(".pid").write_text(str(os.getpid()))
for line in sys.stdin:
    request = json.loads(line)
    if "id" not in request:
        continue
    method = request["method"]
    if method == "initialize":
        result = {
            "protocolVersion": request["params"]["protocolVersion"],
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "owned-smoke", "version": "1"},
        }
    elif method == "tools/list":
        result = {"tools": [{
            "name": "echo", "description": "Return the scoped fixture value",
            "inputSchema": {"type": "object", "properties": {"value": {"type": "integer"}}},
        }]}
    elif method == "tools/call":
        with log.open("a") as output:
            output.write(json.dumps(request) + "\n")
        result = {"content": [], "structuredContent": {
            "scope": os.environ.get("SOURCE_SCOPE"), "cwd": os.getcwd(),
            "input": request["params"]["arguments"],
        }}
    else:
        raise AssertionError(f"unexpected MCP method: {method}")
    print(json.dumps({"jsonrpc": "2.0", "id": request["id"], "result": result}), flush=True)
