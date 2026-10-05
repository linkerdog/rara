# 2026-10-05 Redact env-style secrets

## What was built

`redact_secrets` now catches `GITHUB_TOKEN=...`, `OPENAI_API_KEY=...`,
`AWS_SECRET_ACCESS_KEY=...`, `DB_PASSWORD=...` assignments, `sk-proj-` /
`sk-ant-` keys, and GitHub / Slack / Google API token prefixes.

## Why

The assignment pattern required a word boundary before the keyword. `_` is a
word character, so keywords inside identifiers never matched. The redactor is
shared by transcripts, logs, and the upcoming persistent prompt history, which
writes prompts to disk.

## Trade-offs

Values shorter than 8 characters are still kept, and bare suffixes such as
`tokenizer=` are not treated as secrets, to limit over-redaction.

## Remaining

The redactor is pattern based; high-entropy detection is not attempted.
