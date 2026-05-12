# Local Embedding Runtimes

## Problem

RARA's memory retrieval currently depends on provider-specific embedding calls or
the weak hash fallback exposed through `LlmBackend::embed`. Local-first memory
needs a real embedding path that works when the chat provider does not expose an
embedding endpoint.

The local runtime must also avoid making macOS, Linux, and Windows depend on
the same inference stack. Apple Silicon should use MLX artifacts; other
platforms should start with a CPU ONNX path before any CUDA-specific work.

## Scope

- Add a RARA-owned Python model server under the managed `.rara` home layout.
- Provide a local OpenAI-compatible `/v1/embeddings` endpoint.
- Support macOS Apple Silicon through MLX and Qwen3 Embedding 0.6B.
- Support non-Apple-Silicon platforms through FastEmbed/ONNX and BGE-M3.
- Keep the server bootstrap narrow enough to become one `EmbeddingBackend`
  implementation after embedding provider decoupling lands.

## Non-Goals

- Linux CUDA support.
- Candle-native Qwen3 embedding support.
- Rust-native MLX support through `mlx-rs`.
- Running untrusted Python files from the workspace or from user-selected
  paths.
- Letting arbitrary Hugging Face model ids flow into the bundled model server.
- Changing LanceDB table layout in this slice.

## Architecture

### Canonical Model

The first macOS local embedding profile is:

| Field | Value |
|---|---|
| Profile | `qwen3-embedding-0.6b` |
| Canonical model id | `Qwen/Qwen3-Embedding-0.6B` |
| Embedding dimension | `1024` |
| Max tokens | `8192` for the first local runtime profile |
| Pooling | `last_token` |
| Normalization | L2 normalized |
| Query instruction | `Instruct: ...\nQuery:...` for retrieval queries |

Runtime artifacts are platform-specific:

| Platform | Runtime | Artifact | Status |
|---|---|---|---|
| macOS Apple Silicon | `mlx_qwen3` | `mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ` | First slice |
| Linux / Windows / macOS Intel | `fastembed_bge_m3` | `BAAI/bge-m3` | First slice scaffold |

The canonical model id, dimension, and embedding schema version should drive
future LanceDB table identity. Runtime artifact names must not become the only
storage identity because model families can differ by platform.

### Python Model Server

The runtime is a long-lived Python model server, not a one-request script and
not a Rust `mlx-rs` integration.

RARA bundles the server source in the binary and extracts it to:

```text
~/.rara/runtime/model-server/rara_model_server.py
~/.rara/runtime/model-server/requirements/requirements-macos-arm64.txt
~/.rara/runtime/model-server/requirements/requirements-portable.txt
~/.rara/runtime/model-server/venv/
```

The server binds only to loopback and exposes:

```text
GET  /health
POST /v1/embeddings
POST /models/unload
```

Embedding request:

```json
{"input":"Explain vector search","input_type":"query"}
```

Embedding response:

```json
{"object":"list","model":"mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ","backend":"mlx_qwen3","data":[{"object":"embedding","index":0,"embedding":[0.0]}]}
```

The server lazy-loads embedding backends, restricts model ids to an internal
allowlist, validates the result dimension, and unloads idle models after a fixed
threshold.

RARA should run the server with its own virtual environment. The venv belongs
under the managed model-server runtime directory, not under the workspace:

```text
~/.rara/runtime/model-server/venv/bin/python
```

Dependency installation is a setup operation, not an implicit startup side
effect. Startup should fail with clear guidance when the venv is missing or when
the required backend package is unavailable.

### TUI Status Display

`/status` should expose the local model server as an explicit runtime surface.
The user should be able to tell whether local embeddings are enabled without
reading logs or checking processes.

The status entry should be compact and stable:

```text
● embedding model enabled
  mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ
```

Status states:

| State | Indicator | Meaning |
|---|---|---|
| `enabled` | green | Server is reachable and the selected embedding backend is ready or loaded |
| `setup_required` | yellow | Server script exists but venv or dependencies are missing |
| `disabled` | gray | Local embedding server is not configured for this profile |
| `error` | red | Server failed health check or backend initialization |

The status line should display the selected model name, backend name, and a
short setup or error hint when not enabled. It should not trigger model download
or model initialization just to render `/status`; it should call a lightweight
health endpoint.

### Platform Runtime Selection

The Python server owns platform selection:

```text
macOS Apple Silicon -> mlx_qwen3
otherwise           -> fastembed_bge_m3
```

`mlx_qwen3` uses `mlx_embeddings.load` and `mlx_embeddings.generate`.
`fastembed_bge_m3` uses FastEmbed's ONNX Runtime-backed `TextEmbedding`.
CUDA-specific variants are deferred.

### Health Contract

The model server health response is the source for `/status`:

```json
{
  "ok": true,
  "default_embedding_backend": "mlx_qwen3",
  "platform": {
    "system": "Darwin",
    "machine": "arm64",
    "python": "3.13.0"
  },
  "embeddings": {
    "mlx_qwen3": {
      "backend": "mlx_qwen3",
      "model": "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ",
      "dimension": 1024,
      "loaded": false,
      "last_used": null
    }
  }
}
```

The health endpoint reports readiness and loaded state. Embedding generation
still goes through `/v1/embeddings`.

## Contracts

- The model server is installed only from bytes embedded in the RARA
  binary.
- The model server venv is created only under the managed model-server runtime
  directory.
- Dependency manifests are installed from bundled, platform-specific
  requirements files.
- The server install path must remain inside canonical `~/.rara`.
- Existing server files are reused only when their SHA-256 hash matches the
  bundled bytes.
- Symlinked or non-file server paths must be rejected.
- The server binds only to loopback.
- `/status` must use a lightweight health call and must not force-load model
  weights.
- Status output should show the selected embedding backend and model name when
  local embedding is configured.
- The embedding API accepts text, optional batch text, input type, and a
  backend selector only; it does not
  accept arbitrary module names, local file paths, shell commands, or model ids
  from requests.
- Python dependency installation is not performed implicitly by the server
  installer. Missing dependencies should produce a clear runtime error.

## Validation Matrix

| Case | Validation |
|---|---|
| Server extraction | Unit test installs bundled bytes under a temporary RARA home |
| Requirement extraction | Unit test installs bundled platform requirement manifests |
| Tampered server | Unit test rewrites mismatched server content from bundled bytes |
| Symlink attack | Unix unit test rejects a symlink at the server install path |
| Python protocol | Follow-up integration test calls `/v1/embeddings` and asserts a 1024-d vector |
| Status display | TUI/status test shows enabled/setup/error state without loading model weights |
| macOS runtime | Manual smoke test with `mlx-embeddings` and the MLX Qwen3 artifact |
| Portable runtime | Manual smoke test with `fastembed` and BGE-M3 |

## Operational Notes

- The model server should be invoked by absolute path from RARA's managed runtime
  directory.
- The model server must inherit only the minimum environment needed for model cache
  and Python runtime discovery.
- The server process should use `.rara/runtime/model-server/venv/bin/python`
  after setup.
- `/status` should degrade gracefully if the model server is not running,
  showing setup guidance rather than starting the server implicitly.
- Model downloads are performed by the Python backend dependency stack, not by
  arbitrary workspace code.
- Missing `mlx-embeddings` should be reported as setup guidance, not recovered
  by running `pip install` automatically.

## Open Risks

- The chosen MLX artifact is published with `mlx-lm` metadata, while the sidecar
  uses `mlx-embeddings` for embedding semantics. The first macOS smoke test must
  verify this exact artifact before making it the default.
- Python dependency packaging remains unresolved. A future distribution slice
  must decide whether to create a RARA-managed virtualenv or rely on a user
  Python environment.
- The first portable fallback uses BGE-M3, so mixed-platform embeddings may not
  be vector-compatible with the Qwen3 profile until LanceDB table identity is
  versioned by profile.
- LanceDB table versioning still needs a separate migration plan before mixed
  embedding dimensions or model profiles are enabled.

## Source Journals

- `docs/journal/2026-05-12-local-embedding-sidecar.md`
