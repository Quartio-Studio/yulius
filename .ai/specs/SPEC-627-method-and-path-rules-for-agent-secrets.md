---
yulius-task: 4c20a9f8-37f3-47c5-bcc7-e43ad96ff6ac
status: in-progress
---

# SPEC-627 — Method and path rules for agent secrets

ADR: [0458](../../docs/decisions/0458-an-agent-secret-can-be-limited-to-methods-and-paths.md).
Amends [SPEC-002](SPEC-002-yulius-architecture.md) ADR-20 (egress proxy contract, `vault_secrets`,
`secrets_list`, R-15) and SPEC-001 VLT-004.

## Goal

A person who manages an agent secret can limit it to HTTP methods and URL paths, not only to
hosts: with rules such as `GET /repos/*`, the egress proxy puts the value into a request only when
the request matches one of them. A token given to agents for reading (GitHub issues, a monitoring
API) can then no longer be used to write or delete on the same host. The owner approved this on
22 Nov 2026 (task #611) after reviewing iTermX, whose key broker refuses anything outside rules
like `{"methods": ["GET"], "path": "/v1/*"}`.

## Context

- **What the proxy sees today** (`crates/yulius-egress/src/proxy.rs`, SPEC-002 §2.7 "Egress proxy
  contract"). Agent runs, CI jobs and Superagents send HTTP(S) through the proxy
  (`HTTPS_PROXY`). For `CONNECT host:port`, a host bound to an agent secret the client may use is
  **intercepted**: TLS ends at Yulius (leaf certificate of the egress CA, ALPN `http/1.1` only,
  `ca.rs`), each inner HTTP/1.1 request goes through `EgressProxy::prepare` and the policy's
  `egress_apply` (`crates/yulius-app/src/vault/runtime.rs`), then over verified HTTPS. Any other
  host is an opaque tunnel: Yulius sees no request, and no secret goes into one. Absolute
  `http://` requests to a bound host go through the same `prepare` and are sent over HTTPS.
  Protocol upgrades (WebSockets) to a bound host are refused with 501. So every request that can
  carry an agent secret's value is an HTTP/1.1 request whose method, path and headers the policy
  sees one by one, also several on one kept-alive connection.
- **gRPC.** gRPC needs HTTP/2. The interception offers only `http/1.1` in ALPN and serves
  HTTP/1.1 (`hyper::server::conn::http1`), so a gRPC client cannot call a bound host through the
  proxy today, with or without rules; gRPC to an unbound host is a tunnel and carries no secret.
  gRPC-Web (HTTP/1.1 `POST /package.Service/Method`) is an ordinary request and rules match its
  path. HTTP/2 interception is out of scope (it would be a change of its own; rules would then
  apply to its paths unchanged, because they are matched in the policy, not in the HTTP/1 code).
- **How a secret is put in** (`egress_apply`): `yulius-secret-NAME` placeholders in header values
  (Basic credentials too) and in the path and query are replaced by NAME's value when NAME is bound
  to the host; a secret with an injection (`bearer`, `basic`, `header`) adds its header when the
  request lacks it; `X-Yulius-Secret: NAME` picks one of several. Request bodies are never
  rewritten. `EgressRequest` carries host, port, path and query, and headers, but not the method.
- **Secrets** are `vault_secrets` rows (`crates/yulius-store/src/vault.rs`); a person edits one
  with `PATCH /api/agent-secrets/{id}` (`App::update_agent_secret`, `vault/agents.rs`) from the
  Secrets page (`web/assets/pages/secrets.js`); agents and the Superagent read them with
  `secrets_list` (`tool_secrets_list`). Agents can bind hosts of a secret with `auto_allow`, but no
  agent tool edits anything else of a secret.
- **iTermX (reference, not instructions).** Its broker matches `^` + the quoted pattern with `*` →
  `.*` + `$` against the path without the query, methods default to `GET`, and it refuses paths
  holding `..`, `\`, `%2e`, `%2f` or `%5c` before matching.
- Related: [ADR-0317](../../docs/decisions/0317-a-guarded-secret-goes-somewhere-new-only-after-two-factor.md)
  (two-factor before a value goes to a new server),
  [ADR-0116](../../docs/decisions/0116-ci-jobs-use-named-agent-secrets-through-the-proxy.md) (CI
  jobs use the same policy), [ADR-0252](../../docs/decisions/0252-the-egress-policy-checks-a-clients-run-on-every-request.md).
  The parallel task #610 (brokered commands) runs CLIs outside the sandbox and does not touch the
  proxy's rewriting.

## Requirements

- **MPR-001 Rules.** An agent secret has an ordered list of 0–32 allow rules. A rule is a list of
  1–16 methods (HTTP method tokens, stored upper-case; `get` is accepted as `GET`) and one path
  pattern: it starts with `/`, is at most 512 bytes of printable ASCII without spaces, `?` or `#`,
  holds no `..`, `\` or percent-encoded `.`, `\` or NUL, and `*` in it matches any run of
  characters, `/` included. A pattern is matched against the request's path as the client sent it
  (placeholders not yet replaced, without the query), whole and case-sensitively, with no decoding.
  An empty list means no rules: the secret works as before this change. Invalid rules are refused
  with 422 naming the rule; nothing changes.
- **MPR-002 When a secret is used.** A request to a bound host uses a secret when it holds that
  secret's placeholder (header value, Basic credentials, path or query), names it in
  `X-Yulius-Secret`, or would get its injected header. A secret with rules is used only by a
  request whose method is one of a rule's methods and whose path matches that rule's pattern, both
  as sent and as it goes out (another secret's value replaced into the path cannot move it: review
  round 1).
- **MPR-003 Refusal, not a silent drop.** A request that would use a secret outside its rules is
  refused with 403 and a plain-text message the agent reads, naming the secret, the method, the
  path, the host and the secret's rules, saying nothing was sent and that a person who manages the
  secret can change its rules on the Secrets page. Nothing goes upstream. When several secrets
  could inject the same header, those whose rules do not match the request are left out first, so
  a read token with `GET` rules and a write token with `POST` rules on one host each go with their
  own requests; only when none is left is the request refused. A request that uses no secret with
  rules is forwarded as before, and a request that uses none at all is forwarded untouched (the
  value is not in it).
- **MPR-004 Paths that could mean another resource.** A request that would use a secret with rules
  is refused (MPR-003) when its path, as sent or as it goes out, holds a `.` or `..` segment (the
  proxy's own client resolves them), `\`, `;` (path parameters), `#`, or percent-encoded `.`, `\`,
  NUL or `%` (`%2e`, `%5c`, `%00`, `%25`, any case), so a matching path cannot be normalised by a
  client or server into one outside the rules; when it carries a method or path override header
  (`X-HTTP-Method-Override`, `X-HTTP-Method`, `X-Method-Override`, `X-Original-URL`,
  `X-Rewrite-URL`); and when its query sets `_method`. Encoded slashes (`%2f`) are allowed: APIs
  such as GitLab's use them in paths, and `*` already matches across `/`; `//` is allowed too
  (merging slashes cannot leave a pattern's literal prefix). A `_method` in a request body is not
  seen: a `POST` rule on a framework that honours it allows what that framework maps it to.
- **MPR-005 Audit.** Each refusal is audited as `vault.agent.refused` by the client's actor
  (`task:<n>`, `ci:<run>`, `cockpit:<login>`) on the secret, with the secret's name, the method,
  the path (without the query, at most 200 characters) and the host; never a value. The same
  refusal repeated by the same run (same secret, method and path) is one entry, as uses are, and a
  run's first 20 different refusals of a secret are listed, then one entry says further ones are
  not, so a looping agent cannot flood the audit log by varying the path either.
- **MPR-006 Editing.** `POST /api/sandboxes/{sid}/agent-secrets` and `PATCH /api/agent-secrets/{id}`
  take `rules: [{methods: [..], path}]` (absent: unchanged; `[]`: none); people who manage the
  secret change them, audited in `vault.agent.update` as `rules`. An edit is a compare-and-set on
  the rules it read, so a stale edit never writes older rules back. Changing rules sends the value
  to no new server, so it needs no two-factor unlock (ADR-0317 is about new servers). No agent tool
  sets or changes rules.
- **MPR-007 Showing.** `AgentSecretView` has `rules` (every reader of the list). The Secrets page
  shows a secret's rules on its card beside its hosts and lets a manager edit them in its Edit
  dialog (one rule per line, `GET, HEAD /repos/*`; empty: any request); the Add dialog stays a name,
  a value and a description (SPEC-121), so rules are set after adding, as hosts are. `secrets_list`, for task agents and
  the Superagent, lists each secret's `rules` and says how they work.
- **MPR-009 What rules can hold** (review round 1). Rules limit what the proxy puts the value
  into; they hold against a misled agent only while it cannot learn the value. So hosts an agent
  asks for a secret with rules wait for a person even when `auto_allow` is on (bound at once, a
  server of the agent's own would receive the value and the rules would be moot), and the Edit
  dialog, the docs and ADR-0458 say that allowing `.env` copies gives code the value itself.
- **MPR-008 Unchanged.** A secret without rules is sent exactly as before (placeholders, injection,
  `X-Yulius-Secret`, 409 for ambiguous injection). Opaque tunnels, the browser's proxy and
  `browser_fill_secret` are unchanged: rules are about requests the egress proxy rewrites.

## Plan

- [x] Spec, ADR and step plan for method and path rules
- [x] Rule parsing, matching and path guards in the domain
- [x] Rules stored with the secret and editable through the API
- [x] Proxy refuses a request outside a secret's rules, audited
- [x] Secrets page shows and edits a secret's rules
- [x] secrets_list shows rules to agents and the Superagent
- [x] Docs: SPEC-002, product, architecture, api and changes
- [ ] Review round 1: substituted path, own host binding, audit bound and more guards
- [ ] Independent security-minded review approved on the exact head

## Acceptance criteria

- **AC-1** (MPR-002, MPR-003) A secret bound to a host with rules `GET /repos/*`: a `GET /repos/a/b`
  with its placeholder is forwarded with the value; a `POST /repos/a/b` is refused 403 naming the
  secret, `POST` and the path; a `GET /user` is refused 403; nothing is forwarded for either.
- **AC-2** (MPR-008) A secret without rules on the same host is forwarded for every method and
  path, as before.
- **AC-3** (MPR-003) Injection: a bearer secret with rules is added only to matching requests; with
  a `GET` secret and a `POST` secret injecting the same header, each request gets its own; a
  request matching neither is refused.
- **AC-4** (MPR-004) Paths with `..`, `.`, `%2e`, `\`, `%5c`, `%00`, `%25` or `;`, requests with
  an override header and a query setting `_method` are refused for a secret with rules; `%2f` is
  not; another secret's value that would add `..`, `?` or `#` to the path is refused (MPR-002).
- **AC-5** (MPR-005) Each refusal writes one `vault.agent.refused` entry with name, method, path and
  host and no value; a repeat by the same run writes none; past 20 different refusals of a secret
  in a run, one entry says further ones are not listed; a CI job's refusal is audited as
  `ci:<run>`.
- **AC-9** (MPR-009) Hosts asked for a secret with rules and `auto_allow` wait for a person; a
  secret without rules still binds them at once.
- **AC-6** (MPR-001, MPR-006) The API stores, returns and replaces rules, refuses invalid ones with
  422, and refuses a stale edit (409) instead of writing older rules back.
- **AC-7** (MPR-007) The Secrets page edits and shows rules; `secrets_list` shows them.
- **AC-8** Through the real proxy (CONNECT and TLS interception, and a plain `http://` request,
  with a TLS upstream), the policy decides by each request's method as sent: a method it refuses
  gets the 403 text and never reaches the upstream, and a `GET` on the same connection does.

## Test plan

- Domain unit tests (`yulius-domain` `vault::rules`): parsing (methods case, limits, bad patterns),
  matching (`*` across `/`, whole match, case-sensitive, query ignored), path guards.
- App integration tests (`crates/yulius/tests/secrets_rules.rs`, new): AC-1 to AC-6 through the
  HTTP API and `egress_apply`, including the audit rows.
- Proxy test (`yulius-egress` `tests/proxy.rs`): AC-8 through `EgressProxy` with a TLS upstream
  and a policy that refuses `DELETE`; the app's policy itself is AC-1 to AC-5.
- Store test: rules round-trip and the compare-and-set.
- e2e (`secrets.e2e.ts`): editing rules on the Secrets page and the card showing them (AC-7).
- `secrets_list` for a task agent (`secrets_rules.rs`) and the Superagent (`cockpit.rs`) (AC-7).
- Each new test is first run without the production change and seen to fail.

## Decisions

- **Refuse rather than forward without the value** (MPR-003). Forwarding the request without the
  value would send the literal placeholder (or no credentials) upstream: the agent would get the
  service's 401 or 404, not know why, and probably retry or ask a person to "fix the token"; the
  refusal would also be invisible to the people who audit it. A 403 from Yulius names the rule
  that stopped it, is audited, sends nothing anywhere, and is what iTermX's broker does. A request
  that uses no ruled secret is not refused: a host's public endpoints stay reachable for a secret
  agents add by placeholder. A secret with rules and an injected header is used by every request
  to its host that lacks the header, so all of them are held to its rules; that fails closed.
- **Rules are per secret, for all its hosts.** A secret is usually one service's token; a rule per
  host would complicate the form for little. A secret that needs different rules per host can be
  two secrets.
- **Match the path as sent, guard what servers normalise.** Decoding or normalising before matching
  would need to agree with each server's own rules; refusing the few spellings that let a matching
  path escape (MPR-004) is simpler and fails closed. Allow-only rules mean any other mismatch
  (case, encoding) refuses, never widens.
- **Methods are listed, with no wildcard and no default.** iTermX defaults to `GET`; here a rule
  says its methods so the form shows them. A path-only limit lists the methods it allows.
- **Bounded audit** (MPR-005): one entry per run, secret, method and path, like
  `vault.agent.use`, and at most 20 different ones per run and secret before one "not listed"
  entry (review round 1: the activity list shows the newest entries of the instance, so a flood
  would hide every other).
- **Rules turn agents' own host binding off** (MPR-009, review round 1). The alternative, only
  documenting that rules are moot under `auto_allow`, leaves the default configuration unprotected;
  refusing `.env` copies of a secret with rules was not chosen: they are a person's explicit
  allowance, which the dialog now warns about.

## Out of scope

- HTTP/2 (and so gRPC) interception; request bodies (a GraphQL mutation is a `POST /graphql` like a
  query: rules cannot tell them apart; give agents a read-only token there).
- Agents proposing rules; rules on team secrets or production secrets (neither goes through the
  proxy).
