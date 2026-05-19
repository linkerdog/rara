# 2026-05-19 Local Embedding Prepare Race

## Summary

Local embedding startup now avoids the race between Rust-managed model snapshot
preparation and Python-side eager model loading.

The bundled model server no longer starts a background model preparation job at
process startup. Rust owns the prepare boundary because it may need to pass a
validated local snapshot path to `POST /models/prepare` after ensuring the
allowlisted Hugging Face files are already present in RARA's cache.

Rust startup also keeps polling when `/models/prepare` reports `loading`
instead of immediately returning `PreparingModel`. `PreparingModel` is now used
only after the startup wait window is exhausted while the sidecar still reports
an in-flight load.

Portable FastEmbed/BGE-M3 now uses the same Rust-managed model preparation
boundary as macOS MLX. Rust downloads only the FastEmbed-required
`BAAI/bge-m3` ONNX and tokenizer files into the RARA model cache, records the
snapshot marker, and passes the validated snapshot path to Python. Python then
loads FastEmbed with that local path and `local_files_only`, so Linux startup
does not delegate the initial model download to FastEmbed.

## Background

The previous flow could report `Model · ready` or `Model · already available`
after the Hugging Face snapshot was present, then remain in
`PreparingModel`. That status did not mean the files were still downloading; it
meant the Python sidecar had not yet loaded the embedding backend into memory.

The sidecar made this confusing by starting a background load with no
Rust-provided snapshot path. A subsequent Rust `/models/prepare` call could
therefore see `loading`, return `PreparingModel`, and leave the UI with a
coarse status even though file preparation had already completed.

## Validation

- `cargo test local_model_server::tests::bundled_model_server_does_not_start_background_model_prepare -- --nocapture`
- `python3 -m py_compile components/model_server/rara_model_server.py`
- `cargo check -p rara`
