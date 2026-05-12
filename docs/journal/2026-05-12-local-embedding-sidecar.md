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
processes do not start duplicate servers. If a managed venv already exists and
no reusable server is healthy, the lock owner starts the Python server and writes
fresh metadata. If another process owns startup, later processes report
`waiting_for_server` instead of starting a second server.

This still does not create the venv, install dependencies, or trigger model
download. Those remain the next bootstrap-preparation slice.

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
`~/.rara/runtime/model-server/venv`. Dependency installation is kept as an
explicit setup step rather than an implicit server startup side effect.

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

- Add explicit venv creation and dependency installation during automatic
  bootstrap.
- Add model preparation/download through the Python server and surface progress.
- Wire `/v1/embeddings` into a standalone `EmbeddingBackend`.
- Add config for model server enablement and backend selection.
- Smoke test the exact `mlx-community/Qwen3-Embedding-0.6B-4bit-DWQ` artifact
  with `mlx-embeddings`.
- Smoke test FastEmbed/BGE-M3 on Linux or a non-Apple-Silicon environment.
- Version LanceDB table identity by canonical embedding profile and dimension.
