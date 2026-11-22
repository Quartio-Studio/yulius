# 0458. An agent secret can be limited to methods and paths, and a request outside them is refused

## Status

Accepted on 2026-11-22 for task #611; the owner approved the feature on 22 Nov 2026 after a review
of iTermX's key broker. Specified in
[SPEC-627](../../.ai/specs/SPEC-627-method-and-path-rules-for-agent-secrets.md). Amends
[0014](0014-secrets-vault.md) (SPEC-002 ADR-20: what the egress proxy rewrites) and adds to
[0098](0098-agents-propose-secret-hosts-people-allow-them.md) (hosts are still where a secret may
go; rules narrow what it may do there).

## Context

The egress proxy puts an agent secret's value into any request to a host it is bound to: its
placeholder is replaced, or its header injected, whatever the method and path. A token given to
agents for reading (GitHub issues, a monitoring API) can therefore also write or delete on that
host, by an agent misled by content it read or by anything else that runs in the sandbox.

What the proxy sees: a host bound to a secret is intercepted (TLS ends at Yulius, ALPN
`http/1.1` only), and each inner HTTP/1.1 request reaches the policy's `egress_apply` with its
method, path, query and headers; absolute `http://` requests to a bound host take the same path;
WebSocket upgrades there are refused; every other host is an opaque tunnel that carries no secret.
So every request that can carry a value is one the policy sees whole. gRPC needs HTTP/2, which the
interception does not speak: gRPC to a bound host does not work today, with or without rules;
gRPC-Web over HTTP/1.1 is an ordinary `POST /package.Service/Method`.

## Decision

- An agent secret has 0–32 allow rules, each 1–16 HTTP methods and a path pattern in which `*`
  matches any characters, `/` included. Without rules it is sent as before.
- A request **uses** a secret when it holds its placeholder (header, Basic credentials, path,
  query), names it in `X-Yulius-Secret`, or would get its injected header. A secret with rules is
  used only by a request whose method is listed by a rule whose pattern matches the path as sent
  (placeholders not replaced, query left out, no decoding, case-sensitive).
- A request that would use a secret outside its rules is **refused** with 403 and a text naming
  the request, the host, the secret and its rules; nothing is sent. Of several secrets injecting
  one header, those whose rules do not match are left out first, so a read token and a write
  token on one host each go with their own requests; a request matching none is refused. A
  request that uses no secret with rules is forwarded as before.
- For a secret with rules, a path holding `..`, `\` or an encoded `.`, `\` or NUL, and a request
  carrying a method-override header (`X-HTTP-Method-Override`, `X-HTTP-Method`,
  `X-Method-Override`), are refused: the server could read them as a request outside the rules.
  An encoded `/` is allowed (GitLab's project paths); `*` already crosses `/`.
- Each refusal is audited `vault.agent.refused` with the secret, method, path (no query) and
  host, once per run, secret, method and path, like `vault.agent.use`; never a value.
- People who manage a secret set its rules on the Secrets page (Edit) or with the API; an edit is
  a compare-and-set on the rules it read. Rules send the value to no new server, so changing them
  needs no two-factor unlock ([0317](0317-a-guarded-secret-goes-somewhere-new-only-after-two-factor.md)
  is about new servers). No agent tool sets rules. `secrets_list` shows them.

## Alternatives

- **Forward the request without the value** (the literal placeholder, or no header). The agent
  would get the service's 401 or 404, not know why, and retry or ask a person to fix a token that
  works; nobody auditing would see that a rule stopped it. Refused, as iTermX's broker does.
- **Refuse every request to the host outside the rules**, also those using no secret. Would break
  the host's public endpoints for agents, which need no secret and get none.
- **Normalise or decode the path before matching.** Would have to agree with every server's own
  normalisation; refusing the few spellings that can escape is simpler and fails closed.
- **A default method (`GET`) when none is listed**, as iTermX does. Implicit; the form shows the
  methods instead.
- **Rules per host.** A secret is one service's token; a secret that needs different rules per
  host can be two secrets.
- **Auditing every refused request.** A looping agent would grow the audit log without bound;
  identical repeats in one run are one entry, as uses are.

## Consequences

- A token for reading can be limited to reading. Rules cannot see request bodies: a GraphQL
  mutation and a query are both `POST /graphql`, so a read-only token is still the answer there.
- A secret's rules apply to agent runs, CI jobs and Superagents alike (one policy), and to every
  host it is bound to.
- HTTP/2 interception, if added, inherits the rules unchanged: they are checked in the policy,
  not in the HTTP/1 code.
- Schema version 88 adds `vault_secrets.rules` (JSON, `[]` for every existing secret); a stored
  rule that no longer parses makes the row unreadable rather than unlimited.
