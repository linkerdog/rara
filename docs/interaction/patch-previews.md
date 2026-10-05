# Patch Previews

## Scope

Structured `apply_patch` previews in transcript messages and progress summaries
share this presentation contract. The text producer is shared with the
[pure patch preview API](../features/wasm-core.md). Preview rendering does not
change patch execution, authorization, or persisted transcript fields.

## Contracts

### DIFF-01: Complete File Inventory

Before any hunk, list every affected file in patch order with its operation
(Added, Edited, Deleted, or Moved), source/destination paths, and change counts.
Single-file previews may use their summary as the file header. Counts describe
the complete change, including omitted hunks. A raw deletion directive without
file contents reports an unknown removed-line count; validated tool previews
use the known deleted content. Never imply an unknown count is zero.

### DIFF-02: Per-File Hunk Budgets

Limit each file to 80 source preview lines in the TUI, independently of other
files. File headers and move directives do not consume that budget. Mark each
truncated file with its own exact omitted-line count, including lines already
omitted by the producer. An empty deletion or pure move remains listed with
explicit missing-inline-content feedback. Preview limits count source lines,
including hunk delimiters, rather than terminal rows after wrapping.

### DIFF-03: Width And Content Preservation

Wrap headers, counts, omission markers, and hunk text to the supplied width.
Use shared display sanitation and grapheme wrapping; preserve code whitespace
and keep diff signs distinct from content. Shrink decorative indentation on
very narrow terminals. The summary, tool-result, and committed-transcript paths
must retain the same inventory and per-file limits.

## Verification

- Large first file followed by add, edit, delete, and move operations.
- Multiple truncated files, exact budget boundaries, empty changes, and paths
  containing spaces or wide characters at ordinary and narrow widths.
- Real native dry-run output through runtime formatting and transcript cells,
  plus the shared pure/browser preview producer.
- Existing diff signs and semantic colors; reviewed compact layout snapshots.

## Boundaries

A dedicated full-diff viewer and global tab-stop policy are separate work.
Unstructured fallback snippets retain a bounded preview without claiming a
complete file inventory. Old producer output that already lost file details
cannot reconstruct those details from text alone.

## Source Journal

- [Per-file patch previews](../journal/2026-10-05-per-file-patch-previews.md)
