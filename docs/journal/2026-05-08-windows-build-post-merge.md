# Windows Build After Merge

## Context

The first Windows CI slice added a `build-windows` job to the build workflow.
That made every pull request run a full Windows dependency build, which is more
expensive than the Linux PR gate and not necessary for every review iteration.

## Change

The `build-windows` job now runs only for `push` events. Because the build
workflow only pushes on `main`, Windows compile coverage becomes a post-merge
gate.

Pull requests still run the existing Linux build, fmt, clippy, and test jobs.
The release workflow continues to build Windows release archives and run native
smoke tests where configured.

## Follow-up

If Windows regressions become frequent, add a manual `workflow_dispatch` path or
path-filtered PR Windows builds for platform-sensitive changes.
