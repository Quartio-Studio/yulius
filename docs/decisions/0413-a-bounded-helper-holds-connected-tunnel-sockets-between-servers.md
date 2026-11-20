# 0413. A bounded helper holds connected tunnel sockets between servers

## Status

Accepted for [SPEC-577](../../.ai/specs/SPEC-577-preserve-opaque-egress-tunnels-across-a.md).
The spec's Evidence records which parts of the update path have shipped.

## Context

An update stops the old server and starts a new one. Its namespace holders and frozen agents
can outlive both operations. The accepted Unix relay stream and the TCP streams on the host
belong to the old server. Reconnecting loses the remote peer's position in an opaque stream.

## Decision

Use a Linux helper with one inherited control socket to hold a batch of established connections
during an orderly update. The server selects opaque tunnels and their corresponding relays.
The helper receives connected descriptors and bounded private copy state, then makes the batch
available to one successor through a socket in a service-owned private directory. It uses the
kernel's peer credentials to check the service UID. The old server receives an acknowledgement
when the batch and listener are ready. Admission, destination checks, interception and
credential handling stay in the serving process.

The batch accommodates 256 opaque tunnels and 256 relay pairs. Each pair has up to 32 KiB of
private state. One two-minute deadline covers receipt, adoption and delivery. Each timed
record also carries its original monotonic deadline. Its expiry shuts down its two sockets
throughout the exchange. The successor drains the expired frame and adopts the live remainder.
Once every pair expires the helper releases its only place. On batch expiry,
disconnection or invalid input the helper drops its descriptors and state. During delivery it
retains the original descriptors until the successor acknowledges the entire batch and the
helper confirms adoption. If that exchange fails, the helper shuts down both halves of every
stream. This also closes the connection when a duplicate descriptor is still queued in the
successor's control socket. The socket name
is removed on adoption or orderly exit. Payload bytes live in memory.

## Alternatives

A permanently running proxy would also need current admission policy, host bindings, eviction
accounting and cancellation authority outside the server. Systemd socket activation preserves
listeners, while this stream already has two connected peers and may have a partial write in
each direction. Replaying application bytes on a fresh connection cannot recreate an opaque
TLS session.

## Consequences

The handoff protocol must carry pending bytes, half-closes and accounting when the server paths
are wired. An incompatible successor, an elapsed deadline or a crash before deposit can end a
tunnel. Intercepted traffic reconnects through the successor's host-bound credential checks.
