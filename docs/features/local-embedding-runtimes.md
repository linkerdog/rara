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
- Automatically prepare and start the local model server when local embeddings
  are enabled and no usable server/model is available.
- Reuse an already-running RARA model server across local RARA processes.
- Keep the server bootstrap narrow enough to become one `EmbeddingBackend`
  implementation after embedding provider decoupling lands.
- Route memory embeddings through the local sidecar when the managed server is
  ready, while preserving hosted chat/completion providers for normal turns.

## Non-Goals

- Linux CUDA support.
- Candle-native Qwen3 embedding support.
- Rust-native MLX support through `mlx-rs`.
- Running untrusted Python files from the workspace or from user-selected
  paths.
- Letting arbitrary Hugging Face model ids flow into the bundled model server.
- Changing LanceDB table layout in this slice.
- Adding a user-facing setup subcommand for the first local embedding path.

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
POST /models/prepare
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

Dependency installation and model preparation are startup-managed background
work, not separate user-facing commands. Startup should not block the TUI until
the full model is ready, but it should begin preparation automatically when local
embeddings are enabled and the required venv, packages, server process, or model
artifact are missing.

### Embedding Backend Boundary

RARA has a dedicated embedding backend boundary for local memory retrieval. The
first implementation is a transitional wrapper: chat, tool calling,
summarization, and classification continue to use the selected `LlmBackend`,
while `embed` calls are delegated to the local model server when startup reports
a ready sidecar.

The wrapper exists so existing `MemoryStore`, retrieval, tool, and session
checkpoint call sites can move to local embeddings without a broad constructor
migration. The durable target is still a first-class embedding dependency for
memory/retrieval modules rather than embedding behavior living permanently on
the chat backend trait.

The local embedding client sends OpenAI-compatible requests to the managed
loopback server:

```json
{"input":"Remember this decision","input_type":"document","backend":"mlx_qwen3"}
```

If the sidecar is not ready or startup reports an error, RARA falls back to the
configured provider's existing embedding implementation and surfaces a bootstrap
warning for hard failures.

### Startup Bootstrap And Reuse

RARA owns the model server lifecycle. A normal `rara` startup should:

1. Ensure the bundled model-server component is extracted under
   `~/.rara/runtime/model-server`.
2. Check whether an existing model server is already running by reading the
   managed runtime metadata and calling the loopback health endpoint.
3. Reuse the existing server when the health check succeeds and the reported
   component hash/profile matches the current bundled component.
4. Treat stale metadata, a closed port, a mismatched component hash, or a failed
   health check as a dead server and attempt to become the owner process.
5. Acquire a managed startup lock before creating the venv, installing
   dependencies, starting the server, or preparing the model.
6. Start the server if no reusable server exists.
7. Ask the server to prepare the default embedding backend/model if the health
   endpoint reports that the model is not present or not ready.

The startup lock must live under the managed model-server runtime directory and
must serialize process ownership across multiple RARA instances. Losing the race
to another RARA process is not an error; the later process should wait briefly,
re-read metadata, and reuse the server that the winning process started.

#### Multi-Agent Startup Case

When multiple local agents start at roughly the same time and none finds a ready
model server:

1. Any agent may extract or verify the bundled component, but writes must stay
   hash-checked and atomic.
2. Every agent probes the managed metadata and health endpoint before attempting
   ownership.
3. At most one agent may hold the startup lock and act as the bootstrap owner at
   any point in time.
4. The bootstrap owner creates the venv if needed, installs dependencies, starts
   the server, and asks Python to prepare/download the selected model.
5. Non-owner agents must not create another venv, run another dependency
   install, start another server on a different port, or trigger duplicate model
   downloads while an owner is making progress.
6. Non-owner agents should enter a `waiting_for_server` or `reusing_server`
   state, poll metadata and health with bounded backoff, then attach to the
   owner-started server when it becomes healthy.
7. If the owner dies or the lock becomes stale, waiting agents may compete for
   ownership again. Exactly one replacement owner may proceed, and all remaining
   agents must keep waiting or reuse the replacement server.
8. Ownership transfer requires proof that the health probe still fails and that
   the lock lease is expired or abandoned.

The user-visible status for waiting agents should make the ownership clear:

```text
● embedding model preparing
  waiting for another RARA process to finish mlx_qwen3 setup
```

Once the shared server is healthy, all agents should report the same backend,
model, port, and readiness state.

The managed runtime metadata should contain only local operational state:

```json
{
  "pid": 12345,
  "host": "127.0.0.1",
  "port": 18181,
  "component_sha256": "abc123",
  "profile": "qwen3-embedding-0.6b",
  "started_at": 1778560000
}
```

RARA should not trust metadata alone. Metadata is only a hint for the health
probe; the server is reusable only after the probe succeeds.

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
short setup, download, reuse, or error hint when not enabled. Rendering
`/status` must not itself start a model download or initialize model weights; it
should read the current lifecycle state maintained by startup/bootstrap tasks
and call a lightweight health endpoint when a server is reachable.

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

### Model Preparation And Progress

Model downloads are owned by the Python backend dependency stack because MLX,
FastEmbed, Hugging Face cache behavior, and ONNX Runtime integration all live
behind Python libraries. Rust owns orchestration and display:

- Rust creates or reuses the managed venv.
- Rust starts or reuses the loopback Python server.
- Rust asks the server to prepare a backend/model through a dedicated prepare
  API.
- Python performs package-level model resolution and download.
- Python exposes structured preparation progress.
- Rust polls or subscribes to that progress and surfaces it in `/status` and
  startup status text.

The model server should expose a preparation endpoint:

```text
POST /models/prepare
```

Prepare request:

```json
{"backend":"mlx_qwen3"}
```

Prepare response:

```json
{"ok":true,"backend":"mlx_qwen3","model":"mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ","state":"ready"}
```

Progress should be reported in a small structured shape. Exact transport can be
polling or a stream, but the fields must remain stable:

```json
{
  "state": "downloading",
  "backend": "mlx_qwen3",
  "model": "mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ",
  "message": "downloading model files",
  "downloaded_bytes": 104857600,
  "total_bytes": 524288000
}
```

Valid preparation states:

| State | Meaning |
|---|---|
| `not_started` | No prepare task has started |
| `creating_venv` | Rust is creating the managed Python venv |
| `installing_dependencies` | Rust is installing bundled requirement manifests |
| `starting_server` | Rust is starting the loopback Python process |
| `waiting_for_server` | Another RARA process owns bootstrap and this process is waiting to reuse it |
| `reusing_server` | Rust found and reused an existing healthy server |
| `downloading` | Python is downloading model artifacts |
| `ready` | Server and selected embedding model are ready |
| `error` | Bootstrap, server startup, dependency install, or model preparation failed |

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
- Normal RARA startup should automatically prepare the local embedding runtime
  when local embeddings are enabled and the selected model is missing.
- TUI startup should first inspect the managed sidecar state. If the server and
  selected model are already ready, startup must skip the initialization window
  and avoid repeated venv, dependency, or model preparation work.
- If the sidecar is not ready, TUI startup should open the interface with a
  lightweight provider-backed agent, show an initialization status surface, and
  replace the agent automatically when local embedding bootstrap completes.
- When the local model server is ready, memory embedding calls should use the
  sidecar endpoint instead of the hosted chat provider's embedding fallback.
- Chat completions, tool calling, summaries, context budgets, cache profiles,
  and classifiers remain owned by the configured chat `LlmBackend`.
- Multiple RARA instances should share one healthy local model server instead
  of starting duplicate servers.
- Stale runtime metadata or a dead server process must be detected through a
  failed health probe, then replaced by the process that acquires the startup
  lock.
- `/status` must use a lightweight health call and must not force-load model
  weights.
- Status output should show the selected embedding backend and model name when
  local embedding is configured.
- The embedding API accepts text, optional batch text, input type, and a
  backend selector only; it does not
  accept arbitrary module names, local file paths, shell commands, or model ids
  from requests.
- Python dependency installation is performed by Rust during automatic startup
  preparation from bundled requirement manifests, not by arbitrary server-side
  code or workspace scripts.
- Model artifact download is performed by the Python backend stack after Rust
  asks the managed server to prepare the selected backend/model.

## Validation Matrix

| Case | Validation |
|---|---|
| Server extraction | Unit test installs bundled bytes under a temporary RARA home |
| Requirement extraction | Unit test installs bundled platform requirement manifests |
| Tampered server | Unit test rewrites mismatched server content from bundled bytes |
| Symlink attack | Unix unit test rejects a symlink at the server install path |
| Python protocol | Follow-up integration test calls `/v1/embeddings` and asserts a 1024-d vector |
| Status display | TUI/status test shows enabled/setup/error state without loading model weights |
| Embedding wrapper | Unit test verifies chat calls stay on the configured `LlmBackend` while embeddings use the sidecar backend |
| Local embedding protocol | Unit test verifies the Rust client sends the expected `/v1/embeddings` request shape and parses vectors |
| Startup bootstrap | Unit/integration test starts with no venv/model metadata and records automatic prepare state |
| Server reuse | Integration test starts a second RARA process/client and verifies it reuses the healthy server metadata |
| Dead server recovery | Integration test writes stale metadata or stops the server, then verifies the next startup acquires the lock and starts a replacement |
| Progress reporting | Test or smoke script observes creating/installing/downloading/ready status transitions |
| macOS runtime | Manual smoke test with `mlx-embeddings` and the MLX Qwen3 artifact |
| Portable runtime | Manual smoke test with `fastembed` and BGE-M3 |

## Operational Notes

- The model server should be invoked by absolute path from RARA's managed runtime
  directory.
- The model server must inherit only the minimum environment needed for model cache
  and Python runtime discovery.
- The server process should use `.rara/runtime/model-server/venv/bin/python`
  after setup.
- Startup should prepare the server automatically; `/status` should report that
  background preparation state rather than initiating it.
- TUI startup should not block first paint on local embedding bootstrap. A
  completed sidecar should be reused silently; an incomplete sidecar should show
  initialization progress and close that status surface when the rebuild task
  completes.
- Model downloads are performed by the Python backend dependency stack after
  Rust starts or reuses the managed server.
- Missing `mlx-embeddings` should be recovered by Rust installing the bundled
  macOS requirements into the managed venv. If that installation fails, the
  failure should be reported as setup/bootstrap error state.
- RARA should avoid duplicate model downloads by using the startup lock and by
  reusing a healthy running server.

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
