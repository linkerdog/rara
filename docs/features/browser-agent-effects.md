# Browser Agent Effects

## Problem

Portable contracts can compile for `wasm32-unknown-unknown` while still rejecting
browser-owned values and futures. Native `Send + Sync` bounds reject `Rc` and
JavaScript promises, and native `Instant::now()` cannot provide browser timings.

## Scope

- Shared model, tool, and loop effects running on the browser's local executor.
- Target-selected future/callback bounds with unchanged native thread safety.
- Browser monotonic clocks for inference attempts/calls and memory latency.
- Real headless-browser execution tests and a dedicated CI gate.

## Non-Goals

Provider extraction, HTTP/SSE transports, browser session scheduling, context
assembly, WASI, and worker/worklet support remain separate parts of #871. This
contract does not claim that the native RuntimeSession runs in browsers.

## Architecture

Only `all(target_arch = "wasm32", target_os = "unknown")` selects local futures.
Core platform marker traits retain native `Send`/`Sync` requirements and impose
no thread-transfer requirement in the browser. Streaming callbacks use the same
target selection. Implementors select `async_trait(?Send)` only for this target;
native implementations retain ordinary `async_trait`.

The shared executors continue to await one model/tool/host effect at a time.
They do not spawn threads, select a JavaScript transport, or detach cancelled
work. Browser hosts own cancellation and completion observation just as native
hosts do. `web-time` supplies browser monotonic time; native code retains
`std::time::Instant` without JavaScript dependencies in its production graph.

## Contracts

- Browser backends, tools, policies, and loop effects may retain local state
  across promise suspension. No `unsafe impl Send` or thread-safety wrapper is
  needed to satisfy the portable interfaces.
- Native trait objects and their returned futures retain existing thread-safe
  bounds; callback signatures remain compatible with native implementations.
- Model/tool identity, ordered events and replies, host admission, cancellation
  cleanup, and terminal receipt ordering are unchanged.
- Browser accounting records successful, failed, and dropped/cancelled calls
  and attempts using monotonic elapsed time. Memory timers use the same clock
  boundary. Browser contexts must provide `performance.now()`.
- No wasm feature flag is needed. WASI keeps native bounds and clocks; browser
  support must not accidentally select it through a bare architecture check.

## Validation Matrix

| Boundary | Evidence |
| --- | --- |
| Local futures | Real browser tests retain `Rc` and await JavaScript promises in shared model/tool/loop execution |
| Event ordering | Browser host records streamed model events, tool progress, replies, and finalization in order |
| Admission and stop | Browser tests preserve pause/error/cancellation cleanup boundaries without replay |
| Accounting | Browser success/error/drop paths and memory timer run without native-clock panic |
| Native safety | Compile-time Send/Sync/future assertions and existing native adapter regressions |
| Downstream | Fresh Git consumer compiles native and browser contracts without workspace patches |
| CI | Headless Chrome executes wasm-bindgen tests; compilation alone does not satisfy this gate |

## Open Risks

Browser time precision and suspension behavior depend on the browser. This
boundary does not promise compatibility with worklets lacking Performance or
prove a complete browser provider/session stack. Those remain explicit #871
follow-ups.

## Source Journals

- [Browser agent effects](../journal/2026-10-04-browser-agent-effects.md)
