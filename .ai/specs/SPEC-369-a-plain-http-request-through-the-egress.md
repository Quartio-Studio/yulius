---
yulius-task: fa7a326e-6f91-449d-b623-1908912df463
status: done
---

# SPEC-369: A plain-HTTP request through the egress proxy waits a bounded time for its answer

Status: implemented
Date: 2026-11-01
Owner: Yulius task #352
Scope authorization: task #352, fixes issue #975 (found by the review of task #337, SPEC-355;
verified real by task #348 on dev 50f1faf5). Builds on SPEC-353 (LAP-002, the share), SPEC-355
(SLR-002, connections give way when idle; a plain request is held in flight) and SPEC-357.

## Goal

A plain `http://` request through the egress proxy whose upstream accepts and never answers ends
with `504` after a bounded wait and frees its place in its sandbox's share, so leftover processes
cannot fill the share with requests that never end.

## Context

### Symptom

A plain-HTTP request to an upstream that accepts the connection and never sends a response head
waits forever and keeps one of its sandbox's 128 proxy places. Since SPEC-355 such a request is held
in flight and never gives way to a new connection, so a sandbox's processes can fill its share with
them; every later `CONNECT` of the sandbox is then refused with `503`.

### Root cause

- `EgressProxy::handle`'s plain branch (`crates/yulius-egress/src/proxy.rs`) takes a held place
  (`self.slot(&client, true)`), then awaits `self.send(...)`, and drops the place only once
  `send` returns.
- `send` awaits `builder.send()` on the upstream `reqwest::Client`, which only has
  `connect_timeout` (10 s): once connected, the wait for the response head has no bound. No layer
  above adds one (`serve` is bare hyper http1, and the production entry
  `crates/yulius/src/wiring/egress.rs` calls `proxy.handle` directly).
- The existing tests never used an upstream that accepts and stays silent, so nothing waited on
  the head.

### What else the cause touches

- Intercepted requests (inside a `CONNECT` to a host bound to an agent secret) also call `send`,
  but their place belongs to the tunnel, whose every byte is tracked: an idle intercepted
  connection gives way (SLR-002) and lives at most 24 h. They stay as they are.
- Opaque tunnels never call `send`.

## Requirements

- PHT-001: A plain `http://` request (also one sent over HTTPS to a bound host) whose upstream sends
  no response head for `response_head_timeout` with no progress is answered `504` with a text
  naming the host, and its place is freed. Progress is a request-body frame the proxy forwards
  upstream: each restarts the wait, so a long upload is not cut.
- PHT-002: `EgressConfig::response_head_timeout` sets the bound; the default is `IDLE_FLOOR`
  (300 s), the relays' idle floor.
- PHT-003: The bound covers the wait for the response head only: once the head arrived, the body
  streams with no bound from it (the place is freed before it streams).
- PHT-004: Intercepted requests and tunnels are unchanged: no head bound applies to them.

## Plan

- [x] Spec, root cause and a failing regression test for a silent upstream
- [x] A plain request answers 504 after a bounded wait for its head and frees its place
- [x] A plain request still sending its body, and intercepted requests, are not cut
- [x] SPEC-002 bounds, ADR and changes log
- [x] Independent review, gates green, delivery

## Acceptance criteria

- A plain request to an upstream that accepts and never answers gets `504` after the bound, and
  afterwards the sandbox's full share of tunnels opens again (its places came back).
- A plain request whose body arrives slowly, longer in total than the bound but with gaps shorter
  than it, gets its answer.
- An intercepted request whose upstream answers later than the bound gets its answer; tunnels
  behave as before (the existing tests pass).

## Test plan

In `crates/yulius-egress/tests/proxy.rs`, over real sockets, with a short `response_head_timeout`:

- `a_plain_request_to_a_silent_upstream_gets_504_and_frees_its_place` (PHT-001, PHT-002): a raw
  `TcpListener` upstream that accepts and never writes; the sandbox's whole share of plain
  requests to it each get `504` naming the host, then the whole share of tunnels opens. Fails
  without the fix (the requests never answer; the test's own deadline ends it).
- `a_plain_request_still_sending_its_body_is_not_cut` (PHT-001): a body streamed in frames whose
  gaps are shorter than the bound and whose total is longer gets the echo upstream's `200`.
- `an_intercepted_request_waits_past_the_plain_bound` (PHT-004): a request through an intercepted
  `CONNECT` to an upstream path answering after the bound gets `200`; the same path as a plain
  request gets `504` (the bound is the plain branch's only).

## Decisions

- **The bound is on progress, not the total.** A total bound on `builder.send()` would also bound
  the time to upload the request body; restarting it on each forwarded body frame keeps uploads
  whole while a silent upstream still ends. Alternative rejected: `ClientBuilder::timeout` (cuts
  streaming response bodies and intercepted requests too).
- **300 s by default**, the relays' idle floor: a request no slower than any idle connection the
  sandbox's relays keep, and longer than any call to Yulius waits without a byte.
- **504, not 502**: the upstream was reached and did not answer in time (RFC 9110 §15.6.5).
- **Plain branch only**: intercepted connections already give way when idle and are bounded to
  24 h; a head bound there would change what agents see for slow bound APIs. Recorded in an ADR.

## Review

Independent review in a fresh session (round 1, head e90e04b2): approve, four findings, all own
code, all answered in this change:

- Low: `Instant + Duration` panics for a bound such as `Duration::MAX` (a public field). Fixed
  test-first: `an_endless_head_bound_waits_without_overflow` failed (overflow panic), the deadline
  now uses `checked_add`, and a bound past the clock's range waits with no bound.
- Low: the slow-body test's `read_to_string` had no deadline and a 5x margin. It now has a 30 s
  deadline and a 2 s bound against 200 ms frames (10x), 2.4 s in all.
- Low (process): the spec was still in progress; this section and Evidence complete it.
- Nit: the 504 said "within N s"; it now says "for N s" (the bound restarts with progress). The
  expiry stays a `debug!` line: the share's own warnings already name a full share and who holds
  it (SPEC-353, SPEC-361), and a slow upstream is the agent's to see in its 504, not an operator's.

## Evidence

- Regression first: commit 7d35689d (tests, the config field unused):
  `cargo nextest run -p yulius-egress --test proxy` → 3 failed, 13 passed;
  `a_plain_request_to_a_silent_upstream_gets_504_and_frees_its_place` failed with "a plain request
  to a silent upstream was never answered" (30 s deadline), the plain `/slow` request got 200.
- Fix (75719e66): `cargo nextest run -p yulius-egress` → 24 passed.
- Mutation: `within` ignoring progress (a total bound) → `a_plain_request_still_sending_its_body_is_not_cut`
  failed; restored.
- Gate at e90e04b2: `scripts/dev ci` exit 0; nextest 2932 passed, 16 skipped; clippy, fmt, docs,
  doctests, deps and the macOS build passed.
- After the review fixes: `cargo nextest run -p yulius-egress` → 25 passed;
  `cargo clippy -p yulius-egress --all-targets -- -D warnings` clean; `cargo fmt --check` clean.
- Security scan: Vidoc is turned off for this project in Yulius (disabled, not passed).
- Not tested here: a real 300 s wait (the bound is configurable; the tests use 200 ms–2 s).
