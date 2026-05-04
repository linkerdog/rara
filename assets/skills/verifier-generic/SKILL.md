---
name: verifier-generic
description: Generic verifier template. Copy and customise for your project.
---

# Verifier (Generic)

This is a project-agnostic verifier template. You can copy it to create
project-specific verifier skills (e.g. `verifier-cli`, `verifier-api`).

## Usage

Check the diff and determine the appropriate verification method:

### CLI / Binary
```bash
cargo build && ./target/debug/<binary> --help
./target/debug/<binary> <the-changed-flag-or-subcommand>
```

### TUI
Launch the TUI and observe the changed surface:
```bash
cargo run
```
Capture the rendered output or describe what you see.

### API
```bash
curl -s http://localhost:<port>/<endpoint> | jq .
```

### File Tools
If a file operation changed, run the tool on a test path and verify the output.

## Evidence Capture

- Paste the full terminal output of the verification command.
- Include exit codes.
- Note any unexpected errors, warnings, or behaviour.

## Project-Specific Notes

Add your project's own verification notes below this line.
