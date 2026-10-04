# Browser Agent Effects

## Summary

Continue #871 by making the shared model/tool/loop execution usable on the
browser's local executor and replacing unsupported native clock reads on the
browser target. Native thread-safety constraints and execution behavior remain
intact. Provider/HTTP/context/session extraction remains separate work.

## References And Plan

- Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`,
  `codex-rs/codex-client/src/transport.rs`, retains native Send futures behind a
  transport boundary. Keep that native guarantee rather than relaxing all hosts.
- Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`,
  `src/services/api/client.ts::buildFetch`, injects a host fetch implementation
  independently from request policy. The portable loop similarly accepts host
  effects without choosing a JavaScript transport or spawning work.
- The locally resolved `web-time` 1.1.0 implementation uses browser Performance
  for Instant and exposes standard time on other targets.
- The [wasm-bindgen browser guide](https://wasm-bindgen.github.io/wasm-bindgen/wasm-bindgen-test/browsers.html)
  defines real browser configuration and WebDriver execution. CI uses a matching
  runner version from Cargo.lock and a compatible Chrome/ChromeDriver pair.

The staged plan was: reproduce a local-future compile failure and actual browser
clock panic; adapt only the target-specific bounds and clock boundary; exercise
shared effects in Chrome; verify native compatibility and downstream dependency
graphs; publish with browser CI. Local builds, reversible source edits, temporary
browser tools, and ordinary PR publication use existing task authorization. No
system browser installation or Bazel configuration change is required.

## Key Decisions

- Select the browser with `all(target_arch = "wasm32", target_os = "unknown")`.
  No wasm feature flag or unsafe Send implementation is introduced.
- Core platform marker traits preserve native Send/Sync, while browser traits,
  returned futures, and callbacks can contain Rc/JavaScript-owned values.
- Callback aliases inside async-trait methods bind to its generated
  `'async_trait` lifetime. A minimal compile probe confirmed this preserves
  existing native implementations; anonymous alias lifetimes or a reordered
  explicit callback lifetime change the expanded method contract.
- Native accounting keeps standard Instant. Browser-only `web-time` dependencies
  do not enter the native production graph. Downstream graph checks explicitly
  filter and audit each target instead of treating all Cargo target edges as
  native dependencies.
- Browser tests invoke the actual shared executors and Tool contract. Local
  Promise gates prove cancellation cannot return before pending tool cleanup;
  approval never invokes the paused tool. These are execution tests, not merely
  browser compilation of native Send-compatible fakes.

## Validation

- Baseline local backend failed wasm compilation because Rc and local returned
  futures could not satisfy the old Send/Sync contract.
- Baseline Chrome execution reproduced `time not implemented on this platform`
  in both inference and memory timers.
- Final headless Chrome execution passed three shared-effect tests and two
  accounting/timer tests using Chrome 154.0.8037.92 and wasm-bindgen 0.2.126.
- Native core/agent/observability tests passed (10/32/13), including four new
  Send/Sync/future compile assertions. Strict workspace/all-target Clippy and
  browser-target Clippy for these crates passed with `-D warnings`.
- Native `agent::` regressions passed 182 tests with one existing ignored test;
  six native session-input regressions passed. The first sandboxed agent run
  failed nine compaction tests because its default state directory was read-only.
  Repeating with the supported `RARA_HOME` override in `/tmp` passed.
- Default Bazel core, agent, observability, runtime-session, and embedded-runtime
  test targets passed. The generated module lock changes are limited to the
  added browser/test dependencies and their aliases/input hashes; Bazel
  configuration is unchanged.
- An independent local-path consumer passed four native tests and browser test
  compilation without workspace patches/config/lock. Remote Git validation
  remains a publication gate. Native and browser production graphs are audited
  separately; the native dependency allowlist is unchanged.
- `cargo fmt --all`, explicit Rust formatting for the downstream fixture, and
  `git diff --check` passed.

## Follow-Ups

Extract provider/context assembly and add browser HTTP/SSE transports and session
scheduling before claiming end-to-end browser support. Workers/worklets and WASI
are outside this checkpoint. Exact-head remote CI and review remain delivery gates.
