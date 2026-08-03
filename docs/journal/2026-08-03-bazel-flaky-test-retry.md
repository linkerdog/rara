# Bazel Flaky Test Retry

## What changed

Added a repository-level `.bazelrc` with `test --flaky_test_attempts=3`.
Bazel test invocations now retry a flaky test up to three total attempts by
default.

## Why

Transient test failures should be retried by the test runner before CI reports
the target as failed. Keeping this in the repository configuration makes the
behavior consistent for local and CI Bazel test invocations without requiring
each workflow or developer command to repeat the flag.

The setting applies only to Bazel test commands. It does not change build
actions or hide ordinary test failures: a test that fails on every attempt
still fails.

## Trade-offs

Retries can increase test duration when a test is genuinely flaky. Three total
attempts provide bounded mitigation while keeping repeated failures visible to
CI.

## Verification

- `bazel help test` confirms that Bazel 9.1.1 supports
  `--flaky_test_attempts`.
- Run the affected Bazel test targets without an explicit retry flag and
  confirm the repository configuration is applied.
