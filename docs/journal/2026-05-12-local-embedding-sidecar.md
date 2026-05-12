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

The follow-up embedding-provider decoupling slice also landed on top of this
runtime. Vector-producing paths now use a standalone `EmbeddingBackend`, and
runtime bootstrap routes providers without a reliable embedding surface to the
local model server instead of hashed embeddings. `MemoryStore`, retrieval
orchestration, session-context checkpointing, vector tools, and sub-agents all
inherit that embedding backend, while the configured chat `LlmBackend`
continues to own chat, tool calling, summarization, classification, context
budgeting, and provider cache metadata. The local model server HTTP client also
bypasses system proxies for loopback traffic so local embedding calls do not
leak into proxy-managed outbound routes.

TUI app construction now uses an inspect-only local model server status path.
This keeps `/status` and initial UI state lightweight; they can report setup,
waiting, ready, or error state without creating a venv, installing
dependencies, or loading model weights. Runtime bootstrap remains the owner of
automatic preparation. When the selected provider actually requires the local
sidecar and the managed backend/model is not ready yet, startup opens with a
lightweight agent, starts a rebuild task that performs local embedding
bootstrap, and then automatically swaps in the rebuilt agent when the sidecar
is ready. The rebuild result carries the final local model server status back
into TUI state, so `/status` and the overview panel do not keep showing the
startup inspect-only state after initialization succeeds.

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
- Add explicit embedding provider override / control surface instead of routing
  only from the provider family defaults.
- Surface structured download byte progress from the Python dependency stack
  when the backend exposes it.
- Smoke test the exact `mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ` artifact
  with `mlx-embeddings`.
- Smoke test FastEmbed/BGE-M3 on Linux or a non-Apple-Silicon environment.
- Version LanceDB table identity by canonical embedding profile and dimension.
