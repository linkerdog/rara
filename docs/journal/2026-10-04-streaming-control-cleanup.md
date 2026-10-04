# Streaming Control-Token Cleanup

## Summary

This #923/#921 checkpoint makes assistant control cleanup independent of delta
boundaries and removes source-wide replay for ordinary angle-bracket text.
It also removes the one-use `replace_display_text` helper noted in #955 review:
source replacement stays inside `push_delta`, which updates presentation identity.

## Background And Plan

The previous fast path depended on whether the current delta contained `<` and
whether the accumulated source appeared to have a pending control prefix. It
missed a legacy marker ending in a later `>` chunk, a separator deferred until
after an internal close tag, and bare DSML evidence arriving after a literal
leading think block. Plain text after a scrubbed orphan DSML tail reappeared.
Repeated ordinary angle brackets also rescanned growing source.

Inspected local Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`
(`codex-rs/tui/src/markdown_stream.rs`, accumulated source and explicit commit
boundaries) and Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`
(`src/utils/messages.ts`, complete-message prompt-tag cleanup). The adaptation
keeps source ingestion separate from canonical cleanup instead of teaching
Markdown rendering a second set of control rules.

The plan first reproduced the four chunk failures and ordinary-text replay
cost, then replaced the fast-path predicate with bounded recognition state,
and checked both collector source and live/finalized styled rows against a
fresh canonical collector across every character boundary.

## Key Decisions

- A fixed byte window recognizes internal opens, EOS, and both bare DSML
  evidence spellings. A separate four-state recognizer handles arbitrarily
  long legacy marker names without retaining or rescanning those names.
- Ordinary text and incomplete candidates append directly. Completing a legacy
  marker triggers canonical replay for that delta; subsequent ordinary text
  resumes direct appending. Empty terminal-sanitized deltas do no replay work.
- Internal blocks and DeepSeek evidence select canonical replay for all later
  nonempty deltas in that stream. This deliberately trades performance for
  exact transformation order, retrospective cleanup, delayed separators, and
  persistent orphan-tail suppression. Previously completed controls could
  resume an incorrect direct-append path. This checkpoint does not claim a
  general incremental implementation for those complex contexts.
- The complete-message scrubber is unchanged. The obsolete whole-source
  pending-context predicate and its helper-only test are replaced by production
  stream tests, including the prior punctuation-prefix boundary case.
- No provider events, persistence, public APIs, dependencies, or snapshots change.

## Validation

The five initial regression tests failed before the fix: four on visible text
and one on 133,982,000 source bytes revisited while ingesting ordinary generic,
comparison, and HTML-like text. The latter now revisits zero bytes and records
exactly one recognition visit per input byte. A 60,000-byte unfinished legacy
name stays incremental, replays once on completion, and resumes direct append.

The eight focused cases pass, including 25 fixtures at every UTF-8 character
split and as character-sized deltas. Fixtures cover all internal block tags,
nesting, literal/open think text, late control evidence, both DSML spellings,
valid/malformed calls, orphan tails, transformed marker fragments, ANSI state,
empty deltas, and CRLF. Live and finalized styled rows match fresh collectors.

Cargo and default Bazel TUI suites each pass 980 cases, with three isolated
child fixtures exercised by parent tests. Strict workspace/all-target Clippy,
formatting, and diff checks pass. The first Bazel attempt stopped at missing
external cache packages. Targeted repository fetches restored the active
modules, registry archives, and Rust tools without configuration changes; stale
cached repository names were reported as undefined. The default target then
passed in 154 seconds, including 16 seconds of test execution. The existing
gold-linker deprecation warning is unrelated.

```bash
cargo test --lib tui::
cargo clippy --workspace --all-targets -- -D warnings
bazel test //:rara_unit_tests --test_arg=tui::
cargo fmt --all -- --check
git diff --check
```

## Review Follow-Up

Review follow-up shares DSML evidence spellings with the canonical parser and
adds two focused drift guards. Every internal/EOS/DSML token must select sticky
replay across every character boundary, including tokens added later. Legacy
recognition is compared with canonical cleanup for every ASCII byte in a name,
empty names, and non-ASCII names across every character boundary. Production
grammar and replay costs are unchanged.

Both grammar guards and all eight existing control-stream regressions pass.
The default Bazel control-token target and Cargo format/Clippy commit checks
also pass. The regression filter uses the actual `control_stream_tests` module.

```bash
cargo test --locked --lib control_tokens::
cargo test --locked --lib control_stream_tests::
bazel test //:rara_unit_tests --test_arg=control_tokens::
```

## Follow-Ups

Completed legacy markers and streams containing complex controls retain
source-wide replay costs. A future incremental replacement must preserve all
existing scrub transformations, including their order and malformed-input
behavior. Remote CI/review/merge and physical-terminal acceptance remain
separate gates; #921 and #923 stay open.
