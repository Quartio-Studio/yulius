# 0047. Device sign-in start is limited per client, its grants bounded and purged

## Status

Accepted. Extends SPEC-001 ACC-013; specified in
[SPEC-063](../../.ai/specs/SPEC-063-fix-169-device-sign-in-start-is-rate-lim.md) (issue #169).

## Context

`POST /api/device/start` needs no session (the CLI has none yet), and every call inserted a
`device_grants` row that nothing ever deleted. Anyone who reaches the server could fill its disk
slowly by calling it in a loop, and make every lookup by user code scan a larger table.

## Decision

- Device start is limited per client with the client key of password sign-in (the trusted
  proxy's client address, else the TCP peer; an IPv6 client is its /64): 5 starts per client per
  minute in a sliding window, in separate buckets from sign-in. The limiter tracks at most 4096
  clients, like sign-in's.
- Each grant stores its client key. Live grants are bounded: 10 per client and 1000 per instance.
  A start past either bound is refused with 429; existing grants are never evicted.
- Expired grants (any status) and consumed grants are deleted on every start, in the same
  transaction as the bound check and the insert, and by the background tick every 60 s. Denied
  grants stay until they expire, so the CLI still reads "denied".

## Alternatives

- **Drop the oldest grant past the bound.** A flood would then cancel a real person's approval in
  progress. Refusing only delays new starts, for at most the 10-minute grant lifetime.
- **Count a client's grants in memory.** No schema change, but the bound would not hold across a
  restart. The column costs one migration; the table stays small enough to need no index.
- **Purge only on start, or only on a timer.** On start alone leaves dead rows while nobody
  starts; a timer alone lets dead rows count against the bound between sweeps.

## Consequences

- Schema version 16 (`device_grants.client_key`); an older binary refuses the newer database, as
  for every migration.
- A flood from more than 100 clients (or /64 networks) can keep new device sign-ins refused for
  up to 10 minutes; the web sign-in and existing API tokens are unaffected.
- A purged grant reads like an unknown one. A grant approved but not polled within its 10
  minutes is deleted; polling it reads "expired" and the API token issued for it stays unused
  (revocable in Settings). A denied grant reads "expired" after its 10 minutes, and approving a
  consumed grant's code answers 404 instead of 409. The CLI stops polling at `expires_in_s`.
