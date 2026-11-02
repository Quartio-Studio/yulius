# 0252. The egress policy checks a client's run on every request

## Status

Accepted. Specified in
[SPEC-388](../../.ai/specs/SPEC-388-vidoc-triage-egress-proxy-secret-substit.md); task #370,
Vidoc issues #757 and #833. Amends [ADR-0014](0014-secrets-vault.md) (the egress proxy) and
[ADR-0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md) (a Superagent's bearer is
honoured only while its shell lives, which now holds per request too).

## Context

The egress proxy resolves its caller (`EgressPolicy::egress_client`) once per connection and asks
`egress_apply` how to rewrite each request inside an intercepted connection, which may live up to
24 h (`CONNECTION_MAX`). Two gaps let an ended run keep using agent secrets:

- `egress_client` kept task-token and Superagent clients in a 15 s cache (`CLIENT_CACHE_TTL`), and
  a hit for a task token skipped `agent_context`'s check that the attempt is not over (#757).
- `egress_apply` never asked whether the client's run still lives. So a connection opened by an
  agent run, a CI job or a Superagent kept getting secrets injected after the run ended, the CI
  credential was closed, or the shell exited.

Placeholders are also replaced in the path and query (SPEC-121: some APIs take their key in the
URL). Vidoc (#833) asked to restrict them to headers because URLs get logged. The value goes only to
a host its secret is bound to, the agent sees only the placeholder, the proxy follows no redirect,
and the response is redacted. The one log that recorded such a URL was Yulius's own: a failed
upstream request was logged with reqwest's error text, which holds the full URL.

## Decision

- **`egress_apply` checks the client's run first, on every request** (`App::egress_client_live`):
  an agent run's attempt must exist and not be terminal (one indexed read), a CI job's credential
  must still be open (`ci_clients`, in memory), a Superagent's shell must be running (in memory).
  An ended run gets `403` with "the run behind this proxy credential has ended; nothing was sent";
  nothing is injected, substituted or audited. It is a `403`, not a `407`: inside an intercepted
  connection the client believes it talks to the origin. A run whose state cannot be read gets
  `503` (a passing failure the client may retry), never "ended".
- **`egress_client` answers a client, cached or not, only while its run lives** (the same check),
  so the token of an ended run is refused (`407`) at once, not when the cache entry expires.
- **Placeholders stay allowed in the path and query.** The proxy logs and passes on reqwest errors
  without their URL (`reqwest::Error::without_url`), so Yulius never logs a substituted URL.

## Alternatives

- **Evict the cache where an attempt ends.** Rejected: four `transition_attempt` calls in
  `supervisor.rs` end an attempt, and a new one could forget; the eviction would
  also miss the clients that open connections already hold.
- **Close a run's proxy connections when it ends.** Rejected: it needs a run-to-connections index
  in the proxy and leaves a race between the end and the close; the per-request check costs one
  indexed read, less than the secret read `egress_apply` already does.
- **Substitute placeholders only in headers.** Rejected: it breaks APIs that take a key in the URL
  (SPEC-121); the leak needs a log that records the URL, and Yulius's is fixed instead.

## Consequences

An ended run's credential stops adding secrets at its next request, whatever connection it uses.
Each proxied request to a bound host of an agent run, and each cached `egress_client` hit for
one, costs one more indexed attempt read. A
failed upstream request is logged with its host and the error's kind, not its URL.
