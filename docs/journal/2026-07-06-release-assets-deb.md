# Release Assets And Debian Packages

## Summary

The `.github/workflows/release.yml` release workflow now stages Debian packages
for Linux targets and performs an explicit GitHub Release asset upload after the
release is created. This prevents a tag release from existing without binary
assets attached.

## Scope

- Package `.deb` files for `x86_64-unknown-linux-musl` and
  `aarch64-unknown-linux-musl` using Debian archive names.
- Include `.deb` files in the release checksum set.
- Upload staged release files with `gh release upload --clobber` after
  `softprops/action-gh-release` creates or updates the release.
- Verify the final GitHub Release asset count.

## Validation

```bash
ruby -e 'require "yaml"; YAML.load_file(".github/workflows/release.yml"); puts "yaml ok"'
git diff --check
```
