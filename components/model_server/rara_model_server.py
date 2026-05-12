#!/usr/bin/env python3
"""RARA local model server.

The server intentionally uses the Python standard library for HTTP plumbing so
RARA can bootstrap it before optional ML dependencies are available. Model
backends are loaded lazily and are restricted to an internal allowlist.
"""

from __future__ import annotations

import gc
import json
import os
import platform
import sys
import time
from http import HTTPStatus
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any, Iterable


DEFAULT_HOST = "127.0.0.1"
DEFAULT_PORT = 18181
MAX_BODY_BYTES = 1_000_000
IDLE_UNLOAD_SECONDS = 600

MLX_MODEL_ID = "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ"
FASTEMBED_MODEL_ID = "BAAI/bge-m3"
EMBEDDING_DIMENSION = 1024
MLX_MAX_TOKENS = 8192
QUERY_INSTRUCTION = (
    "Given a web search query, retrieve relevant passages that answer the query"
)


def _json_response(
    handler: BaseHTTPRequestHandler, status: HTTPStatus, payload: dict[str, Any]
) -> None:
    data = json.dumps(payload, separators=(",", ":")).encode("utf-8")
    handler.send_response(status)
    handler.send_header("Content-Type", "application/json")
    handler.send_header("Content-Length", str(len(data)))
    handler.end_headers()
    handler.wfile.write(data)


def _read_json(handler: BaseHTTPRequestHandler) -> dict[str, Any]:
    raw_length = handler.headers.get("Content-Length")
    if raw_length is None:
        raise ValueError("missing content length")
    try:
        length = int(raw_length)
    except ValueError as exc:
        raise ValueError("invalid content length") from exc
    if length < 0 or length > MAX_BODY_BYTES:
        raise ValueError("request body size out of bounds")
    body = handler.rfile.read(length)
    request = json.loads(body)
    if not isinstance(request, dict):
        raise ValueError("request body must be a JSON object")
    return request


def _platform_default_backend() -> str:
    if platform.system() == "Darwin" and platform.machine().lower() in {"arm64", "aarch64"}:
        return "mlx_qwen3"
    return "fastembed_bge_m3"


def _format_query(text: str, input_type: str) -> str:
    if input_type != "query":
        return text
    return f"Instruct: {QUERY_INSTRUCTION}\nQuery:{text}"


def _first_vector(values: Iterable[Any]) -> list[float]:
    try:
        vector = next(iter(values))
    except StopIteration as exc:
        raise RuntimeError("embedding backend returned no vectors") from exc
    if hasattr(vector, "tolist"):
        vector = vector.tolist()
    out = [float(value) for value in vector]
    if len(out) != EMBEDDING_DIMENSION:
        raise RuntimeError(f"unexpected embedding dimension: {len(out)}")
    return out


class EmbeddingBackend:
    name = "unknown"
    model_id = ""

    def __init__(self) -> None:
        self.loaded_at: float | None = None
        self.last_used = 0.0

    def load(self) -> None:
        raise NotImplementedError

    def embed_one(self, text: str, input_type: str) -> list[float]:
        raise NotImplementedError

    def unload(self) -> None:
        self.loaded_at = None
        gc.collect()

    def unload_if_idle(self) -> None:
        if self.loaded_at is None:
            return
        if time.monotonic() - self.last_used >= IDLE_UNLOAD_SECONDS:
            self.unload()

    def status(self) -> dict[str, Any]:
        return {
            "backend": self.name,
            "model": self.model_id,
            "dimension": EMBEDDING_DIMENSION,
            "loaded": self.loaded_at is not None,
            "last_used": self.last_used if self.last_used else None,
        }


class MlxQwen3Backend(EmbeddingBackend):
    name = "mlx_qwen3"
    model_id = MLX_MODEL_ID

    def __init__(self) -> None:
        super().__init__()
        self.model: Any | None = None
        self.tokenizer: Any | None = None

    def load(self) -> None:
        if self.model is not None and self.tokenizer is not None:
            return
        requested = os.environ.get("RARA_MLX_EMBEDDING_MODEL", MLX_MODEL_ID)
        if requested != MLX_MODEL_ID:
            raise RuntimeError(f"unsupported MLX embedding model: {requested}")
        from mlx_embeddings import load

        self.model, self.tokenizer = load(requested)
        self.loaded_at = time.monotonic()

    def embed_one(self, text: str, input_type: str) -> list[float]:
        self.load()
        assert self.model is not None
        assert self.tokenizer is not None
        import mlx.core as mx
        from mlx_embeddings import generate

        prepared = _format_query(text, input_type)
        output = generate(
            self.model,
            self.tokenizer,
            [prepared],
            max_length=MLX_MAX_TOKENS,
            padding=True,
            truncation=True,
        )
        embeddings = output.text_embeds
        mx.eval(embeddings)
        self.last_used = time.monotonic()
        return _first_vector([embeddings[0]])

    def unload(self) -> None:
        self.model = None
        self.tokenizer = None
        super().unload()
        try:
            import mlx.core as mx

            mx.metal.clear_cache()
        except Exception:
            pass


class FastEmbedBgeM3Backend(EmbeddingBackend):
    name = "fastembed_bge_m3"
    model_id = FASTEMBED_MODEL_ID

    def __init__(self) -> None:
        super().__init__()
        self.model: Any | None = None

    def load(self) -> None:
        if self.model is not None:
            return
        requested = os.environ.get("RARA_FASTEMBED_EMBEDDING_MODEL", FASTEMBED_MODEL_ID)
        if requested != FASTEMBED_MODEL_ID:
            raise RuntimeError(f"unsupported FastEmbed model: {requested}")
        from fastembed import TextEmbedding

        kwargs: dict[str, Any] = {"model_name": requested}
        cache_dir = os.environ.get("RARA_MODEL_CACHE_DIR")
        if cache_dir:
            kwargs["cache_dir"] = cache_dir
        self.model = TextEmbedding(**kwargs)
        self.loaded_at = time.monotonic()

    def embed_one(self, text: str, input_type: str) -> list[float]:
        self.load()
        assert self.model is not None
        self.last_used = time.monotonic()
        if input_type == "query" and hasattr(self.model, "query_embed"):
            return _first_vector(self.model.query_embed([text]))
        if input_type == "document" and hasattr(self.model, "passage_embed"):
            return _first_vector(self.model.passage_embed([text]))
        return _first_vector(self.model.embed([text]))

    def unload(self) -> None:
        self.model = None
        super().unload()


class ModelRegistry:
    def __init__(self) -> None:
        self.backends: dict[str, EmbeddingBackend] = {}

    def embedding_backend(self, requested: str | None) -> EmbeddingBackend:
        name = requested or os.environ.get("RARA_EMBEDDING_BACKEND") or _platform_default_backend()
        if name not in {"mlx_qwen3", "fastembed_bge_m3"}:
            raise ValueError(f"unsupported embedding backend: {name}")
        if name not in self.backends:
            if name == "mlx_qwen3":
                self.backends[name] = MlxQwen3Backend()
            else:
                self.backends[name] = FastEmbedBgeM3Backend()
        return self.backends[name]

    def unload_idle(self) -> None:
        for backend in self.backends.values():
            backend.unload_if_idle()

    def status(self) -> dict[str, Any]:
        default_backend = _platform_default_backend()
        return {
            "ok": True,
            "default_embedding_backend": default_backend,
            "platform": {
                "system": platform.system(),
                "machine": platform.machine(),
                "python": sys.version.split()[0],
            },
            "embeddings": {
                name: backend.status() for name, backend in sorted(self.backends.items())
            },
        }


REGISTRY = ModelRegistry()


class RequestHandler(BaseHTTPRequestHandler):
    server_version = "RARAModelServer/0.1"

    def log_message(self, format: str, *args: Any) -> None:
        sys.stderr.write(format % args + "\n")

    def do_GET(self) -> None:
        if self.path == "/health":
            _json_response(self, HTTPStatus.OK, REGISTRY.status())
            return
        _json_response(self, HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})

    def do_POST(self) -> None:
        try:
            if self.path == "/v1/embeddings":
                self._create_embeddings()
                return
            if self.path == "/models/unload":
                self._unload_model()
                return
            _json_response(self, HTTPStatus.NOT_FOUND, {"ok": False, "error": "not found"})
        except Exception as exc:  # noqa: BLE001
            self.log_message("Error handling POST %s: %s", self.path, exc)
            _json_response(self, HTTPStatus.BAD_REQUEST, {"ok": False, "error": str(exc)})
        finally:
            REGISTRY.unload_idle()

    def _create_embeddings(self) -> None:
        request = _read_json(self)
        raw_input = request.get("input")
        if isinstance(raw_input, str):
            inputs = [raw_input]
        elif isinstance(raw_input, list) and all(isinstance(item, str) for item in raw_input):
            inputs = raw_input
        else:
            raise ValueError("input must be a string or list of strings")
        if not inputs:
            raise ValueError("input must not be empty")

        input_type = request.get("input_type", "document")
        if input_type not in {"query", "document"}:
            raise ValueError("input_type must be query or document")

        backend_name = request.get("backend")
        if backend_name is not None and not isinstance(backend_name, str):
            raise ValueError("backend must be a string")
        backend = REGISTRY.embedding_backend(backend_name)
        data = []
        for index, text in enumerate(inputs):
            data.append(
                {
                    "object": "embedding",
                    "index": index,
                    "embedding": backend.embed_one(text, input_type),
                }
            )
        _json_response(
            self,
            HTTPStatus.OK,
            {
                "object": "list",
                "model": backend.model_id,
                "backend": backend.name,
                "data": data,
            },
        )

    def _unload_model(self) -> None:
        request = _read_json(self)
        backend_name = request.get("backend")
        backend = REGISTRY.embedding_backend(backend_name if isinstance(backend_name, str) else None)
        backend.unload()
        _json_response(self, HTTPStatus.OK, {"ok": True, "backend": backend.name})


def main() -> int:
    host = os.environ.get("RARA_MODEL_SERVER_HOST", DEFAULT_HOST)
    port = int(os.environ.get("RARA_MODEL_SERVER_PORT", str(DEFAULT_PORT)))
    if host not in {"127.0.0.1", "localhost"}:
        raise RuntimeError("RARA model server only binds to loopback hosts")
    server = ThreadingHTTPServer((host, port), RequestHandler)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        server.server_close()
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
