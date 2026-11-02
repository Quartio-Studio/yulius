---
yulius-task: e6ae5fe5-27d4-4314-ac5d-5d59e56fe88c
status: in-progress
---

# SPEC-388: Vidoc triage of the egress proxy's secret substitution (#839 #709 #833 #708 #757)

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #370
Scope authorization: the person's goal for task #370: close the five Vidoc issues #839, #709,
#833, #708 and #757, each as a false positive (comment with file:line evidence, closed as not
planned; a duplicate labelled `duplicate`) or as solved by one pull request into `dev` with a
`Fixes #<n>` line per real issue. Create no new GitHub issue.

## Goal

Every one of the five findings is confirmed or refuted against `dev` (4e07ce4e). The real ones are
fixed test-first, and the false positives get narrow `[[ignore]]` entries, so a later scan does
not reopen their issues.

## Context

The egress proxy (`crates/yulius-egress/src/proxy.rs`) intercepts requests to hosts bound to an
agent secret and asks the application's policy (`EgressPolicy` in
`crates/yulius-app/src/vault/runtime.rs`) who the caller is (`egress_client`, once per proxy
connection) and how to rewrite each request (`egress_apply`, per request inside an intercepted
connection). Triage, with the evidence posted on each issue:

| Issue | Finding | Verdict |
| --- | --- | --- |
| #839 | CR/LF in a secret value injects headers | False positive: refused at write (`check_agent_value`, `yulius-domain/src/vault/mod.rs:75`), set aside at send (`transportable`, runtime.rs:1056-1066), `HeaderValue::from_bytes` refuses CR/LF (proxy.rs:994-1001); tests `values_with_control_characters_are_never_sent`, `the_egress_policy_never_sends_a_value_with_a_line_break`. Closed as not planned. |
| #709 | CR/LF in headers and the request line | Duplicate of #839 (closed as not planned, labelled `duplicate`); a space in the path is percent-encoded by reqwest's URL parser. |
| #708 | Hop-by-hop and framing headers forwarded | False positive: `prepare` drops `HOP_BY_HOP` and `Connection`-listed headers before `egress_apply` (proxy.rs:56-68, 928-946), refuses `Upgrade` (921); hyper's server drops `Content-Length` when `Transfer-Encoding` is present and refuses conflicting lengths, and its client frames the body itself. Closed as not planned. |
| #833 | Secret values in the request URL leak to logs | Real in one place: substituting into the path and query is a documented, tested feature (SPEC-121; `tests/secrets.rs` CI test, `tests/proxy.rs` query placeholder) needed by APIs that take keys in the URL, and the value goes only to the bound host (the agent sees the placeholder, redirects are not followed, responses are redacted). But when the upstream request fails, the proxy logs reqwest's error at debug level (proxy.rs:1036), and that error's text holds the full upstream URL, value included. |
| #757 | A cached client keeps an ended run's token valid for 15 s | Real, and wider than reported: a cache hit skips the attempt-state check `agent_context` makes (coordination.rs:100), and the client of an open intercepted connection, which may live 24 h (`CONNECTION_MAX`), is never checked again. So an ended agent run, a closed Superagent shell or an ended CI job keeps getting agent secrets injected on a connection it already holds. |

## Requirements

- **EPL-001** (#757) `egress_apply` refuses (403, nothing forwarded, nothing injected or audited)
  a request whose client's run has ended: an agent run whose attempt is terminal or missing, a CI
  job whose credential was closed, a Superagent whose shell is gone. A run whose state cannot be
  read gets 503 (a passing failure), never "ended".
- **EPL-002** (#757) `egress_client` never answers a cached client whose run has ended; the check
  is the same as EPL-001's, so a token of an ended run is refused (407) at once, not after
  `CLIENT_CACHE_TTL`.
- **EPL-003** (#833) No log line of the proxy holds the upstream URL: reqwest errors are logged and
  passed on without it (`reqwest::Error::without_url`).
- **EPL-004** The four false-positive wordings (#839, #709, #708, and the design part of #833) get
  one narrow `[[ignore]]` entry each in `.config/vidoc.toml`, titled with Vidoc's exact wording
  and saying which guard stops it.

## Plan

- [x] Triage the five findings against dev and close the false positives with evidence
- [ ] Ended runs' proxy clients add no secret, per request and on a cache hit
- [ ] The proxy never logs a substituted upstream URL
- [ ] Vidoc ignores, ADR-0252 and docs record the triage
- [ ] Independent review approved and findings answered
- [ ] Gates green with evidence in the spec

## Acceptance criteria

- `an_ended_runs_proxy_client_adds_no_secret` (crates/yulius/tests/secrets.rs) fails on dev (a
  cached `egress_client` and a held client still get `Authorization: Bearer <value>` after the
  attempt ended or the CI credential closed) and passes with the fix.
- `a_failed_upstream_request_never_logs_the_secret_in_its_url` (crates/yulius-egress/tests/proxy.rs)
  fails on dev (the debug line holds `for url (https://…?key=<value>)`) and passes with the fix.
- `scripts/dev ci` is green.

## Test plan

- Integration (App, real store): seed a live attempt, resolve its client (cached), apply a request
  to a bound host (injected), end the attempt with `transition_attempt`, then assert the cached
  `egress_client` answers `None` and `egress_apply` with the held client refuses 403 without an
  `authorization` header; the same for a CI client after `ci_egress_close`. A live run still gets
  its secret (no false refusal).
- Proxy end to end (real sockets, TLS upstream): the upstream closes the connection without an
  answer for `/fail`; the agent gets 502, and the captured debug log has the failure line without
  the value.
- The Superagent's liveness check is the existing one (`self.shell(..)`), moved into the shared
  check; its existing tests (`tests/cockpit.rs`) cover it.

## Decisions

- **Check liveness per request, not only per connection** (ADR-0252). Alternatives: evicting the
  cache at each of the four places an attempt ends (fragile: a new end path forgets it, and it
  would not reach clients already held by open connections); closing a run's proxy connections
  when it ends (needs a run-to-connections index in the proxy for the same effect, and still a
  race between the end and the close). One indexed attempt read per proxied request costs less
  than the secret read `egress_apply` already does.
- **Keep placeholders in the path and query** (ADR-0252). Restricting them to headers would break
  APIs that take a key in the URL (SPEC-121); the leak Vidoc describes needs a log that records
  the URL, and the only one in Yulius is fixed by EPL-003.
- 403 (not 407) for an ended run inside an intercepted connection: the client believes it talks to
  the origin there, and a 407 from an origin is meaningless to it; the message says why.
