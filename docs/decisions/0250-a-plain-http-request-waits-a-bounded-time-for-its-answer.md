# 0250. A plain-HTTP request waits a bounded time for its answer

## Status

Accepted. Specified in
[SPEC-369](../../.ai/specs/SPEC-369-a-plain-http-request-through-the-egress.md); task #352, issue
#975. Builds on [ADR-0241](0241-a-full-relay-closes-its-longest-idle-connection-and-the-superagent-restarts-in-place.md)
and [ADR-0243](0243-the-busiest-run-gives-way-at-a-full-proxy-share.md).

## Context

The egress proxy holds a plain `http://` request's place in its sandbox's share in flight until the
upstream's response head arrives: it passes through no tracked stream, so it never gives way
(ADR-0241, ADR-0243). The upstream client had a connect timeout only, so a request whose upstream
accepted and never answered kept its place for good, and a sandbox's leftover processes could fill
its share with such requests.

## Decision

- **A plain request waits at most `EgressConfig::response_head_timeout` (300 s, the relays' idle
  floor) with no progress for the upstream's response head**, then is answered `504` naming the
  host, and its place is freed.
- **Progress is a request-body frame forwarded upstream**: each restarts the wait, so an upload
  longer than the bound is not cut while it moves.
- **Only the head is bounded**: the place is freed before the response body streams, and the body
  has no bound from this.
- **Intercepted requests and tunnels are unchanged**: their place is their connection's, whose bytes
  are tracked, so it gives way when idle and lives at most 24 h.

## Alternatives

- **`reqwest::ClientBuilder::timeout`.** Rejected: it bounds the whole response, cutting streaming
  bodies, and applies to intercepted requests too.
- **A total bound on the wait for the head.** Rejected: it would also bound a slow upload.
- **Letting a held plain request give way when idle.** Rejected: the request has no tracked bytes,
  and closing it would cut a slow but live upstream with no answer to the client.
- **`502` on expiry.** Rejected: the upstream was reached and did not answer in time, which is
  `504` (RFC 9110 §15.6.5).

## Consequences

A sandbox's processes can no longer fill its share with plain requests that never end: each frees
its place within 5 minutes of its last progress. An upstream that takes longer than 5 minutes to
start answering a plain-HTTP request now gets `504`; through HTTPS (a tunnel or an intercepted
connection) it is not bounded.
