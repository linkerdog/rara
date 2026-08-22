# AppServer Architecture

The canonical session ownership, command, snapshot, and replay contract lives
in `runtime-session.md`. AppServer is a transport adapter over `RuntimeHost`; it
must not own raw agents or reconstruct lifecycle semantics.

## Current Boundary

RARA currently provides two pieces of the AppServer boundary:

- `rara-app-server` contains transport-neutral control request and provenance
  contracts;
- the root library exposes `RuntimeSession`, ordered control events, replay,
  and the non-global multi-session `RuntimeHost` used by ACP.

Ask, print, exec, Wire, embedded, and ACP execution use the canonical session
handle. The TUI still uses its internal `RuntimeClient` compatibility owner.
There is no network AppServer listener in the current checkpoint.

## Target Architecture

```text
                    +---------------- RuntimeHost ----------------+
connection ingress -> session command routing -> RuntimeSession(s)
                    |                         -> snapshot + replay |
                    +---------------------------------------------+
                                      |
                                      v
                          bounded connection egress
```

The transport owns connection authentication, negotiated capabilities,
subscription cursors, and outbound backpressure. `RuntimeHost` owns the
process-local mapping from stable runtime session IDs to cloneable session
handles. Each `SessionActor` owns the mutable runtime graph for exactly one
session.

Transport disconnect does not implicitly destroy a host-owned session. A
reconnecting client supplies a cursor, consumes bounded replay, or receives
`ResyncRequired` and fetches a new snapshot.

## Adapter Contract

An AppServer adapter may:

- authenticate a request and select an authorized session;
- translate a supported control request to one `RuntimeSession` command;
- subscribe from a snapshot and serialize ordered `RuntimeControlEvent`
  values;
- apply independent ingress and egress queue bounds;
- remove or shut down a session only when the host lifecycle requests it.

It must not:

- retain a mutable `Agent`;
- assign a second event sequence or synthesize tool-call IDs by tool name;
- accept tenant, workspace, or authorization identity from model-generated
  tool arguments;
- make memory the owner of runtime sessions;
- retry an ambiguously dispatched provider request after observable output or
  a tool side effect.

## Delivery History

| Status | Change |
| --- | --- |
| done | ACP, print, and Wire adapters consume the shared runtime event model. |
| done | `RuntimeSession` owns bounded command, cancellation, terminal, snapshot, and replay behavior. |
| done | ACP uses `RuntimeHost` for independent sessions without a process-global registry. |
| pending | Move the TUI compatibility command processor behind `RuntimeSession`. |
| pending | Add a concrete network transport only when an external deployment requires it. |
