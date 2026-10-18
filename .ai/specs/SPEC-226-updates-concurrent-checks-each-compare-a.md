---
yulius-task: 3d99033d-cd02-4f1b-962d-7da97ec115a0
status: in-progress
---

# SPEC-226: Concurrent update checks compare once; an older update read never lands last

Status: in-progress
Date: 2026-10-18
Owner: task #212
Scope authorization: task #212: "Fix GitHub issue #502". Fixes #502, found in the review of
SPEC-149 (task #143). Extends SPEC-149 and SPEC-002 ADR-18; no new decision record (a bug fix
inside the existing design).

## Goal

Two update checks sent together make one GitHub compare, and the update view in the browser
always shows the newest read, whatever order the answers arrive in.

## Context

Confirmed against `dev` at a3b9b04a.

1. **Concurrent checks.** `check_updates_now` (`crates/yulius-app/src/updates.rs`) decides whether
   the last check is younger than 30 s (`MANUAL_CHECK_GAP_MS`) *before* `check_releases` takes the
   `checking` lock, and `check_releases` never reads `check` again. Two checks sent together (two
   tabs opening the instance page, which checks by itself since SPEC-149, or a page opening while
   the hourly round checks) both see a stale result, queue on the lock, and each run a compare.
   The existing test (`crates/yulius/tests/self_update.rs`) only sends checks one after another.
2. **Older read lands last.** `refresh()` in `crates/yulius-server/web/assets/updates.js` assigns
   every `GET /api/update` answer to `view`. It runs on the `update` event, on the stream's
   reconnect, on the poll timer, on load and after the instance page's actions, so reads overlap;
   an answer to an older read arriving after a newer one shows the older state until the next
   change is announced. No test held two reads and answered them out of order.

## Requirements

- **UPC-001** A manual check that waited for another check reuses the result stored meanwhile when
  it is younger than 30 s; it does not compare again. The hourly round always compares.
- **UPC-002** The browser applies a `GET /api/update` answer only when no answer to a later read
  has been applied; an older answer arriving last is dropped. A failed read changes nothing.

## Plan

- [x] Spec and plan for #502
- [x] Concurrent manual checks share one compare (fake source gate, regression test, fix)
- [x] An older /api/update answer arriving last is dropped (mock per-read hold, e2e test, fix)
- [ ] Docs, gates, review and delivery

## Acceptance criteria

- `concurrent_checks_make_one_compare` (self_update.rs): with a fake source that blocks inside
  `compare`, a check held in it and a second check sent meanwhile end with one compare and the
  same result; no sleeps (the test waits on a "compare entered" signal and the lock's queue).
  Fails on `dev`.
- `an older update read answering last does not replace a newer one` (update.e2e.ts): two held
  reads, the newer (no offer) answered and applied first, the older (offer) answered after; the
  offer stays hidden. Fails on `dev`.

## Test plan

- Rust: `FakeSource` gets an optional gate: `compare` reports that it entered and waits for a
  release (`tokio::sync::Notify`). Once the first (spawned) check entered, the second is polled
  once: nothing before the lock awaits, so it is past its freshness read and queued on the lock.
  The test then releases the gate, awaits both and counts compares.
- e2e: the mock gets `holdUpdateReads`: each `GET /api/update` snapshots its view on arrival and
  waits until `releaseUpdate()` (newest first, one per call); `updateReadsTaken` counts held
  answers whose body the page has read, so the test knows the page handled an answer before
  asserting (everything after `response.text()` resolves in microtasks).
- Existing self_update and update e2e tests keep passing.

## Decisions

- Re-check freshness after taking the lock with a `reuse_within: Option<i64>` argument on
  `check_releases` (`Some(MANUAL_CHECK_GAP_MS)` from `check_updates_now`, `None` from the hourly
  round) rather than single-flight sharing of an in-flight future: the lock already serializes
  compares, so reading the slot again under it is the smallest change. The early read before the
  lock stays as a fast path that avoids waiting on the lock at all.
- A request sequence in `refresh()` rather than serializing reads (one in flight, one queued):
  overlapping reads stay as fast as today, and the rule "never go back to an older read" is
  local and needs no queue. Dropping is by request order, not by `checked_at`, because the view
  also carries the run and the phase, which have no common timestamp.
- The mock snapshots a held read's view when it arrives (as `holdGitReads` does), since the shared
  `hold=` builds the answer on release and could never show an older state landing last.
