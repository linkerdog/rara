"""Execute candidate calls and report observations, without contract expectations."""

import contextlib
import errno
import json
import os
from pathlib import Path
import sys
import types


def observe(candidate, request):
    arguments = request["arguments"]
    originals = [
        item
        for argument in arguments
        if isinstance(argument, list)
        for item in argument
    ]
    try:
        value = getattr(candidate, request["function"])(*arguments)
    except MemoryError:
        raise
    except BaseException as error:
        record_error = getattr(candidate, "RecordError", None)
        line_number = getattr(error, "line_number", None)
        return {
            "error": {
                "value_error": isinstance(error, ValueError),
                "record_error": isinstance(record_error, type)
                and isinstance(error, record_error),
                "line_number": line_number if type(line_number) is int else None,
                "json_decode_cause": isinstance(error.__cause__, json.JSONDecodeError),
            }
        }
    return {
        "error": None,
        "value": value,
        "arguments": arguments,
        "aliases_arguments": [value is argument for argument in arguments],
        "item_origins": (
            [
                next(
                    (i for i, original in enumerate(originals) if item is original),
                    None,
                )
                for item in value
            ]
            if isinstance(value, list)
            else None
        ),
    }


def execute(workspace, source_path, requests):
    observe_call = observe
    encode, decode = json.dumps, json.loads
    candidate = types.ModuleType("candidate")
    candidate.__file__ = str(workspace / "task.py")
    sys.modules[candidate.__name__] = candidate
    try:
        with open(os.devnull, "w") as sink, contextlib.redirect_stdout(
            sink
        ), contextlib.redirect_stderr(sink):
            source = source_path.read_text()
            exec(compile(source, candidate.__file__, "exec"), candidate.__dict__)
            # Snapshot each observation before later calls can mutate shared objects.
            observations = [
                decode(encode(observe_call(candidate, request))) for request in requests
            ]
    except MemoryError:
        return {"resource_limit": True}
    except BaseException:
        return {"candidate_error": True}
    return {"observations": observations}


if __name__ == "__main__":
    try:
        requests = json.load(sys.stdin)
        print(
            json.dumps(execute(Path(sys.argv[1]), Path(sys.argv[2]), requests)),
            flush=True,
        )
    except MemoryError:
        sys.exit(75)
    except OSError as error:
        if error.errno == errno.EFBIG:
            sys.exit(75)
        raise
