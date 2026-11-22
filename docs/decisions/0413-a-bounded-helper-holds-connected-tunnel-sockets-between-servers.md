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
when the batch and listener are ready. Orderly shutdown starts capture and deposit alongside
HTTP shutdown. The two operations have fifteen seconds and five seconds respectively.
Admission, destination checks, interception and
credential handling stay in the serving process.

The batch accommodates 256 opaque tunnels and 256 relay pairs. Each pair has up to 32 KiB of
private state. One two-minute deadline covers receipt, adoption and delivery. Each timed
record also carries its original monotonic deadline. Its expiry shuts down its two sockets
throughout the exchange. The successor drains the expired frame and adopts the live remainder.
When both FINs complete, Linux can forget the TCP peer name while the original socket still
has its final bytes or EOF to deliver. A timed copy can record its two kernel socket cookies
while both endpoints have peers. The sender completes an identity prefix without transferring
descriptors. Local capture and deposit check the stream roles, identities and state size before
taking custody. The receiver verifies the roles and identities, then takes custody so the
original deadline applies while it reads the size and state. It checks the size before
allocating the state buffer. A validated terminal queue can drain its final bytes and any
bytes in the saved copy cursor. If a stream resets or times out, the remaining pairs can
still transfer.
Once every pair expires the helper releases its only place. On batch expiry,
disconnection or invalid input the helper drops its descriptors and state. During delivery it
retains the original descriptors until the successor acknowledges the entire batch and the
helper confirms adoption. If that exchange fails, the helper shuts down both halves of every
stream. This also closes the connection when a duplicate descriptor is still queued in the
successor's control socket. The socket name
is removed on adoption or orderly exit. Payload bytes live in memory.

Under a delegated Linux service, the helper joins the sessions unit's service-owned legacy
leaf. If that unit is unavailable and the server unit delegates controllers to its children,
retaining the helper there requires a systemd version that can restart that layout. Failed
placement ends the handoff before acknowledgement so the server can restart.
[ADR-0298](0298-sessions-live-in-their-own-unit.md)

## Alternatives

A permanently running proxy would also need current admission policy, host bindings, eviction
accounting and cancellation authority outside the server. Systemd socket activation preserves
listeners, while this stream already has two connected peers and may have a partial write in
each direction. Replaying application bytes on a fresh connection cannot recreate an opaque
TLS session.

## Consequences

An established opaque HTTP/1.1 CONNECT retains its connected sockets, pending bytes,
write-half state, admission owner, byte activity and absolute expiry. In namespace mode
the server also transfers the enrolled host-side relay. At CONNECT the proxy verifies both
live internal endpoints and records the kernel cookie of the relay socket. Each unclaimed
relay can hold one duplicate descriptor for this check, bounded by its ordinary admission
limit. Enrollment releases the duplicate. The successor verifies the recorded cookie against
the actual descriptor and checks the surviving holder's identity. Duplicate records for one
enrolled socket are discarded. The replacement reserves distinct relay sockets in original
admission order, including sockets whose internal TCP addresses were reused. Eviction,
an abandoned checkpoint or administrative cancellation releases both enrolled copies, even
when the external peers keep their write halves open. A FIN leaves the opposite direction
available for a final reply. When one
enrolled copy has finished both write halves, it retains its descriptors and admission
place until the matching copy drains or the connection is cancelled or expires. Thus a
restart can still transfer application bytes queued on the last active copy.

Continuity requires compatible handoff support in both server versions. HTTP/2 inside
the opaque tunnel is ordinary payload. An outer HTTP/2 CONNECT shares its transport
and HTTP session state with other streams, so it reconnects. A lost holder, an elapsed
deadline, or a crash before deposit can end a tunnel. Intercepted streams reconnect
through the successor's host-bound injection and redaction policy.
