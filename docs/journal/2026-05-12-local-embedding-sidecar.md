# 2026-05-12 Local Model Server

## What Changed

Defined the first local embedding runtime slice around a RARA-owned Python
model server:

- macOS Apple Silicon uses MLX with Qwen3 Embedding 0.6B.
- Other platforms use FastEmbed/ONNX with BGE-M3 as the first portable path.
- Candle, Linux CUDA, and Rust-native MLX are deferred.

Added a bundled Python model server scaffold and a Rust extractor that writes
the embedded server and platform requirement manifests under RARA's managed home
directory.

The extractor is now called from TUI startup, so the bundled component is no
longer dead code. `/status` reports the selected local embedding backend, model
name, and whether setup is still required.

The Rust side now also owns the first process-discovery boundary. Startup reads
managed `server.json` metadata, verifies the loopback `/health` identity, reuses
a matching server, and uses a non-blocking `startup.lock` so multiple RARA
processes do not start duplicate servers. The lock owner creates the managed
venv when it is missing, installs the selected bundled requirements manifest,
starts the Python server, writes fresh metadata, and asks the server to prepare
the selected embedding backend through `POST /models/prepare`. If another
process owns startup, later processes report `waiting_for_server` instead of
starting a second server or triggering a duplicate dependency/model prepare.

The Python server now reports the default embedding backend in `/health` before
weights are loaded, tracks preparation state, and exposes `POST
/models/prepare`. Rust treats a server as reusable only after the health
identity matches and the selected backend reports `loaded: true`.

RARA now has a dedicated embedding backend boundary for this sidecar. Runtime
bootstrap prepares the managed model server and wraps the configured chat
backend when the sidecar is ready: chat, tool calling, summarization,
classification, context budgeting, and provider cache metadata still go to the
configured `LlmBackend`, while `embed` calls go to the local sidecar. If the
sidecar is unavailable, RARA falls back to the provider embedding path and
reports bootstrap warnings only for hard local embedding errors.

TUI app construction uses an inspect-only local model server status path. This
keeps `/status` and initial UI state lightweight; they can report setup,
waiting, ready, or error state without creating a venv, installing dependencies,
or loading model weights. Runtime bootstrap remains the owner of automatic
preparation.

TUI startup now mirrors the same idempotent check: it first inspects the managed
sidecar. If the selected backend/model is already ready, startup skips the
initialization task. If not, the UI opens with a lightweight provider-backed
agent, starts a rebuild task that performs local embedding bootstrap, and then
automatically swaps in the rebuilt agent when the sidecar is ready.

## Why

The Rust MLX ecosystem is not yet the lowest-risk path for a Qwen3 embedding
runtime, especially with 4-bit MLX artifacts. A Python model server keeps the
first slice small while still letting RARA own the installed script bytes,
install path, hash validation, process boundary, and HTTP API.

This mirrors the useful part of Nowledge Mem's design: a native app supervises
a local Python backend, while model loading stays lazy inside that backend.

## Safety Boundary

The server is extracted from bytes compiled into RARA. The extractor refuses
symlinked server paths, repairs modified files by restoring bundled bytes, and
keeps the install path under `~/.rara/runtime/model-server`.

The server binds only to loopback. It accepts embedding API requests only and
does not accept local script paths, shell commands, arbitrary imports, or
arbitrary model ids.

The intended Python runtime is a RARA-owned venv under
`~/.rara/runtime/model-server/venv`. Dependency installation is performed by the
Rust bootstrap owner from bundled requirement manifests, then recorded through a
requirements hash marker so ordinary startup can skip a repeated install.

## Status Surface

`/status` now shows local embedding model state with backend, model, setup
detail, waiting state, startup state, and reused endpoint when available. Ready
state is backed by a lightweight health probe; rendering `/status` itself does
not initiate download or model weight loading.

Target shape:

```text
● embedding model enabled
  mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ
```

## Follow-Up

- Add config for model server enablement and backend selection.
- Move bootstrap work off synchronous TUI startup so creating the venv,
  installing dependencies, and downloading model artifacts can stream progress
  without blocking first paint.
- Move `MemoryStore`, retrieval, and tools from the transitional `LlmBackend`
  embedding wrapper to an explicit embedding dependency.
- Split query/document embedding intent in the Rust memory path so retrieval
  queries can use the Python server's query instruction path.
- Surface structured download byte progress from the Python dependency stack
  when the backend exposes it.
- Smoke test the exact `mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ` artifact
  with `mlx-embeddings`.
- Smoke test FastEmbed/BGE-M3 on Linux or a non-Apple-Silicon environment.
- Version LanceDB table identity by canonical embedding profile and dimension.
