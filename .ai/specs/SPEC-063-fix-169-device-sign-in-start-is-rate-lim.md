---
yulius-task: 8a6dbf2a-4f03-49f4-8cf8-c1e074afd792
status: in-progress
---

# SPEC-063: device sign-in start is rate limited, bounded and purged

Status: in-progress
Date: 2026-10-13
Scope authorization: task #55 (part of task #47, "fix all open actionable GitHub issues"): fix
GitHub issue #169 following its acceptance criteria. Extends SPEC-001 ACC-013 and SPEC-002 §9.

## Goal

`POST /api/device/start` can no longer grow the database without bound: starts are limited per
client, the live device grants are bounded per client and instance-wide, and expired and
consumed grants are deleted.

## Context

- `POST /api/device/start` (`setup_auth::device_start` → `App::device_start`,
  `crates/yulius-app/src/auth.rs`) needs no session (the CLI has none yet) and inserts one
  `device_grants` row per call. Nothing ever deletes a row (#169).
- Password sign-in is already limited per client (ACC-013): the `ClientAddress` extractor
  (`crates/yulius-server/src/client.rs`) gives the trusted proxy's address, else the TCP peer,
  and `limit_key` folds an IPv6 client to its /64. `App::password_login` keeps a sliding
  one-minute window per key in memory, at most 4096 keys.
- `oauth_states` already purges expired rows on insert (`insert_oauth_state`).
- `App::start_background` runs a 60 s tick for queue draining.
- The CLI (`yulius login`, `yulius onboard`) starts one grant per sign-in and retries a 429 a few
  times before showing the error (`crates/yulius-import/src/http.rs`).

## Requirements

- R1. Device start is limited per client key (the same `ClientAddress::limit_key` as password
  sign-in): 5 starts per client per minute (a burst of 5 for real CLI use), sliding window. Past
  it the start answers 429 "too many device sign-ins; wait a minute". The limiter tracks at most
  4096 client keys; keys idle for a minute are forgotten first; while 4096 are active a new key
  also waits. Its buckets are separate from password sign-in's.
- R2. Each grant records its client key (`device_grants.client_key`, schema version 16).
- R3. Live grants (rows left after the purge, R4) are bounded: at most 10 per client key and
  1000 per instance. A start past either bound is refused with 429 and inserts nothing; existing
  grants are never evicted, so a flood cannot cancel someone's approval in progress.
- R4. Purge: expired grants (any status) and consumed grants are deleted on every start (in the
  same transaction as the bound check and the insert) and by the background tick every 60 s.
  Denied grants stay until they expire so the CLI's poll still reads "denied".
- R5. The device flow is unchanged otherwise: start, approve and poll behave as before; polling a
  purged grant reads "expired", as polling a consumed or unknown one did.

## Plan

- [x] Spec, ADR and limits recorded in SPEC-001 and SPEC-002
- [x] Device start limited per client with a shared sliding-window limiter
- [x] Grants record their client and are bounded per client and instance-wide
- [ ] Expired and consumed grants purged on start and every minute
- [ ] Independent review, gates green, changes entry and evidence

## Acceptance criteria

- AC1. Over a real listener with the proxy secret configured, one proxied client's sixth start
  within a minute gets 429 while another proxied client starts; a minute later the first starts
  again.
- AC2. With the limiter out of the way (different clients), the 11th live grant of one client and
  the 1001st of the instance are refused with 429 and the table holds at most the bound.
- AC3. Expired and consumed rows are gone after one purge (the background sweep's call) and after
  the next start; a denied, unexpired row stays.
- AC4. The existing start → approve → poll test still passes (token returned once).

## Test plan

- Integration (`crates/yulius/tests/trusted_proxy.rs`): AC1 over the real listener.
- Integration (`crates/yulius/tests/http_auth.rs`): AC2, AC3 through `App` with the test clock;
  AC4 the existing device approval test.
- Store (`crates/yulius-store/tests/store.rs`): insert with bounds and the purge delete the right
  rows.

## Decisions

- Refuse past the bound instead of dropping the oldest: dropping lets a flood cancel a real
  person's approval in progress; refusing only delays new starts for at most the 10-minute TTL.
  Recorded in ADR-0042.
- The per-client bound is counted in the database (a `client_key` column) rather than in memory,
  so it holds across restarts; the table is at most 1000 rows, so no index is needed.
- Purge on start plus a 60 s sweep: on-start alone leaves rows when nobody starts, a timer alone
  lets the table reach the bound with dead rows between sweeps.
