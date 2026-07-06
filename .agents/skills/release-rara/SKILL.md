---
name: release-rara
description: Use when preparing, validating, tagging, or troubleshooting a RARA release. Enforces the release order that prevents tag versions from diverging from the Cargo package version.
---

# Release RARA

Use this skill for every RARA release request, including version bumps,
release-candidate checks, tag creation, workflow dispatch, and release failure
triage.

## Non-Negotiable Rule

Never create, push, or dispatch a release tag until the Cargo package version
matches the target tag version.

For tag `vX.Y.Z`, `[workspace.package].version` in `Cargo.toml` must be
`X.Y.Z`. For prerelease tag `vX.Y.Z-alpha.N` or `vX.Y.Z-beta.N`, the Cargo
version must be `X.Y.Z-alpha.N` or `X.Y.Z-beta.N`.

If the release workflow reports:

```text
Tag version X.Y.Z does not match Cargo package version A.B.C.
```

stop the release. Do not rerun the release workflow until a normal commit bumps
the Cargo version to `X.Y.Z` and that commit is the one being tagged.

## Release Preparation Checklist

1. Confirm the requested tag and version.
   - Tag format: `vX.Y.Z`, `vX.Y.Z-alpha.N`, or `vX.Y.Z-beta.N`.
   - Version is the tag without the leading `v`.
2. Start from an up-to-date `main`.
   - Check the worktree is clean before editing.
   - Pull `main` before preparing the version bump.
3. Update the Cargo version before tagging.
   - Edit `[workspace.package].version` in `Cargo.toml`.
   - Update `Cargo.lock` by running `cargo metadata --locked` first to detect
     whether the lock is already consistent; if it is stale, run a normal Cargo
     command that refreshes the lockfile.
4. Verify the exact package version.
   - Use the `rara` package by name, not the first metadata package.
5. Commit the version bump.
   - Commit title format follows RARA rules, for example:
     `chore: bump version to X.Y.Z`
6. Push the version bump and wait for required CI.
7. Create and push the tag from the version-bump commit.
8. Watch the release workflow until assets are published and verified.

## Required Version Check

Run this before creating or pushing a release tag:

```bash
VERSION=X.Y.Z
cargo metadata --no-deps --format-version 1 \
  | python3 -c 'import json,os,sys; data=json.load(sys.stdin); pkg=next(p for p in data["packages"] if p["name"]=="rara"); actual=pkg["version"]; expected=os.environ["VERSION"]; print(actual); sys.exit(0 if actual == expected else 1)'
```

If this exits non-zero, fix `Cargo.toml` before continuing.

## Tag Commands

Only after the required version check passes:

```bash
TAG=vX.Y.Z
git tag -a "${TAG}" -m "Release ${TAG}"
git push origin "${TAG}"
```

For a manual workflow dispatch, the tag must already exist and point to the
commit with the matching Cargo version.

## Existing Bad Tag Recovery

If a tag already exists on the wrong commit:

1. Do not force-push or delete the tag without explicit user approval.
2. Create a normal version-bump commit on `main`.
3. Explain that the existing tag points at the wrong versioned commit.
4. Ask whether to create a new patch tag or to move the existing tag.

Prefer a new patch tag when the bad tag may already have been observed by
GitHub Actions, package consumers, or release automation.
