# Terminal Capability Fallback

## Background

Issue #982 reports unconditional RGB colors and Unicode glyphs under
`NO_COLOR`, limited-color terminals, `TERM=dumb`, and non-UTF-8 locales.
Theme-token lookups alone cannot cover existing constants, syntax colors,
widget borders, and retained transcript rows.

## References And Plan

Inspected Codex `f959e7fc9832dfa0ebfb6542ab1bbf829638ac24`:
`codex-rs/tui/src/terminal_palette.rs`, `tui.rs`, and `styles.md`. The applicable
pattern is cached startup detection with a single palette conversion owner.
Inspected Claude Code `4b9d30f7953273e567a18eb819f4eddd45fcc877`:
`src/ink/colorize.ts` and `src/native-ts/color-diff/index.ts`. Color-depth
adaptation belongs at the output boundary and must preserve explicit no-color
intent. The [NO_COLOR convention](https://no-color.org/) treats a nonempty value
as disabling color without disabling other text attributes.

1. Extend the existing terminal-detection crate with a pure environment-derived
   profile. Prove precedence and explicit ASCII/Unicode profiles with isolated
   tests before connecting startup. No dependency or persisted-format changes.
2. Add the theme palette converter and shared ASCII glyph table, then project
   completed frames. Prove column-width preservation and modifier retention
   before connecting all production frames. Keep the rich-rendering path cheap.
3. Bind the profile once at terminal startup and verify actual TUI buffers and
   terminal output across profiles. Check editing/copy fidelity and narrow
   layouts, then run focused tests, the full TUI suite, and strict Clippy.

The alternative of adapting each renderer independently was rejected because
it duplicates policy and can miss syntax or widget output. Projection is
bounded by visible cells and preserves source text, cache content, and cursor
geometry. Nominal ANSI palette matching cannot discover user-remapped terminal
colors; dynamic palette probing is outside this issue.

Local edits, builds, network verification, feature pushes, and PR creation are
already authorized. Default Bazel configuration and cache remain unchanged.

## Validation

- `cargo test --locked -p rara-terminal-detection`: nine passed, including
  explicit suppression, five color levels, and locale-precedence cases.
- `cargo test --locked --lib tui:: -- --nocapture`: 1055 passed, four existing
  ignored tests. The presentation matrix covers 240 combinations of startup
  or transcript/overlay surfaces, two viewport sizes, and ten color/glyph
  profiles. Rich output is restored byte-for-byte in the buffer after testing
  degraded profiles, proving retained rows remain unchanged.
- `cargo test --locked --lib tui::custom_terminal::color -- --nocapture`: two
  passed after retaining the existing native Windows fallback.
- `cargo clippy --locked --workspace --all-targets --no-deps -- -D warnings`:
  passed. Exact-color test fixtures have item-level expectations; production
  renderers retain the raw-color gate.
- `cargo fmt --all -- --check` and `git diff --check`: passed.

The output oracle verifies actual basic/indexed SGR, selected colors, ASCII-only
bytes, separate bold/dim intensities with reverse, cursor columns, and replacing
a two-column placeholder without leaving a stale cell. The vt100 model treats
bold and dim as alternative intensities, so each is asserted separately.
The shared palette unit test also verifies both modifier bits survive buffer
projection. Input editing and selection extraction retain original Unicode.

## Implementation Decisions

- Terminal startup binds the immutable detected profile to its `TuiApp`.
  Offscreen constructors retain an explicit unrestricted profile; tests inject
  profiles without changing ambient process environment.
- Final-frame projection covers semantic and legacy palette values, syntax
  highlighting, cached text, borders, and overlays. A per-frame color map avoids
  repeating nearest-palette searches, and the unrestricted path returns early.
- ASCII replacement preserves occupied columns. Recognized UI symbols get a
  shared ASCII counterpart; other clusters use width-preserving placeholders.
  Source text, cached rows, input editing, and selection stay in Unicode.
- Inspection of local crossterm 0.29 showed that even named colors are encoded
  as 256-color SGR. The terminal adapter now encodes named colors with basic
  SGR and emits extended codes only for already-resolved extended colors.
  The old native Windows color conversion is retained under `cfg(windows)`.
- Quantization can collapse a distinct foreground/background pair, so the
  palette owner restores text contrast in that case. Palette conversion never
  changes text modifiers.

## Follow-Ups And Limits

Required remote CI/review, including default Bazel, remain merge gates. The
previously recorded local external `rules_rust` cache failure is unchanged.
The environment profile does not probe user-remapped ANSI colors. `TERM=dumb`
selects monochrome ASCII presentation; the interactive surface still requires
cursor addressing and does not become a separate line-mode client. Physical
terminal behavior and the native Windows path were not exercised here.

## Main Integration And Windows Review

Preserved both capability binding and diagnostic-reader initialization when
merging current main. A focused regression reproduced monochrome ASCII output
for Windows Terminal without Unix TERM/locale variables. Following the existing
Codex Windows Terminal hint and Claude Code explicit-suppression precedence,
startup detection now fills missing values with RGB and Unicode on Windows or
with a nonempty WT_SESSION. Explicit color, TERM, and locale restrictions retain
priority. Platform and environment are injected in tests, without process-global
environment mutation. Native Windows console acceptance remains a separate check.

Integration validation: terminal-detection crate 12 tests passed; the merged TUI
suite passed 1,116 tests with seven parent-driven child fixtures ignored.
