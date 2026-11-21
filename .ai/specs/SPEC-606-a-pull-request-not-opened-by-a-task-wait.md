---
yulius-task: 5975c9fc-b3d6-49b9-aaa3-9126e2d851ad
status: in-progress
---

# SPEC-606 — First CI run for a pull request opened outside Yulius

## Goal

Queue an eligible pull request at its exact head within a minute of opening, without increasing the GitHub App's hourly discovery budget.

## Context

The reported interval on 21 November 2026 for quartio-v2 #1545 is about 510 seconds, from opening at 20:31:40 UTC to the queue record at 20:40:10 UTC. All 510 seconds precede worker admission. #1548 and #1549 entered the queue in their opening second. Task delivery and review invoke an immediate sandbox sync. An opening through gh had no such trigger. The previous App manifest subscribed to no events. A hand-opened PR could also be picked up incidentally by another task’s sandbox sync. The queue timestamp alone cannot identify which sync found #1545.

In the checked-out revision before this change, the periodic loop defaulted to 60 seconds (configurable from 10 to 3600). It awaited every sandbox in sequence, each sandbox's sync lock, Git fetch, paginated PR listing, planning and status publication, then up to 50 status retries and pruning before its next round. A head absent from the fetch waits for a later round. At the default interval, the tick alone explains at most 60 seconds of a healthy round. That leaves roughly 450–510 seconds of additional elapsed work or missed or failed rounds, unless the deployed interval differed. The report of quota exhaustion elsewhere that day cannot locate a back-off in this interval. The GitHub client bounds GET retries to three attempts and a 30-second budget and returns an error when the quota reset is too far away. This task's sandbox has no service journal for the incident and its repository credential cannot read quartio-v2. The historical poll setting, individual stage durations and any rate-limit response during this interval are unverified.

## Requirements

- **PRC-001** On delivery of a signed opening for an open, same-repository PR against the configured integration branch, schedule its exact head immediately. The one-minute target assumes timely GitHub delivery and available Git, storage and server capacity. A worker starts when a lane is free. GitHub accepting the pending status is a separate write subject to its quota and the existing status retries.
- **PRC-002** Use the event and existing clone to discover the head. Preserve existing gate selection, SHA sharing, pending status publication and periodic recovery. Duplicate deliveries and concurrent syncs must reuse the run. Preserve the existing 30-open-PR limit per sandbox. When full, admit waiting PRs during reconciliation as places become available.
- **PRC-003** Reject unsigned or incorrectly signed payloads. Ignore unrelated actions, unknown installations or repositories, forks, wrong bases, stale heads, inactive sandboxes and disabled CI. Bound inbound work before reading a request body or consulting stored credentials. Existing stored App credentials must still load.

## Plan

- [x] Account for the opening-to-queue delay
- [x] Discover outside PR heads within one minute
- [x] Prove queue timing, deduplication and request cost
- [ ] Approve the verified change for delivery to dev

## Acceptance criteria

- **AC-1** A signed opening queues the ordinary PR run and publishes its pending status before any periodic sync, even when listing PRs fails. A stalled list or outstanding status write for the same sandbox leaves the webhook free to queue. The stale list cannot cancel the new run, and a later fresh list can reconcile a closure.
- **AC-2** Repeating the delivery and reconciling the sandbox creates one run. Invalid signatures, forks, other bases and stale heads produce no runs.
- **AC-3** The App manifest subscribes to PR events and its conversion secret stays sealed. An existing installation can enable the endpoint. A missed delivery remains discoverable by the regular sync. Slow request bodies time out and release admission capacity. The 31st eligible PR enters the next inventory with a vacancy, at its original head if unchanged.
- **AC-4** Independent review, green PR-head CI, merge into dev and one staging request are recorded by Yulius.

## Test plan

Use the real HTTP router, Git clone, CI store and fake GitHub. Assert the queued ref and SHA, request counts, replay, filtering and authentication. Run the manifest and touched CI tests, quick contracts and the project's security command. Record the manual mutation and machine delivery evidence.

## Decisions

A signed push supplies the missing trigger and repository identity without charging a PR-list read or changing the poll interval. A Git pull ref alone cannot prove the base branch or distinguish a fork. Existing App owners enable delivery under their App registration and provision the matching server secret. The signed opening establishes the PR state at event time. A close or retarget after opening is reconciled by the next fresh inventory. A delivery older than a minute falls back to the inventory. At most 16 deliveries hold a slot from before the first body read through scheduling and status publication. A slow body has three seconds to finish. The sandbox scheduling lock protects local decisions. Status writes proceed after its release. Both admission paths share a window of 30 open PRs per sandbox. An admitted PR retains its place until inventory observes its closure or ineligibility. Inventory fills vacancies from the other eligible PRs. On restart the scheduler restores active PR runs before admitting openings. The HTTP response waits up to three seconds for an immediate error. A slower worker continues after acknowledgement. [ADR-0441](../../docs/decisions/0441-github-pr-openings-trigger-exact-head-ci.md)

## Evidence

- The reported opening and queue times differ by about 510 seconds. The task sandbox's service journal had no entries for 20:30–20:41 UTC, the project session search found no record of pr/1545 in that interval, and the task-scoped GitHub credential could not resolve quartio-v2. The Context records the limits of the historical attribution.
- The GitHub adapter’s manifest conversion test failed on the old empty event list. It passed with the PR subscription, endpoint and conversion secret.
- With one sandbox lock held by the stalled PR-list request, `an_outside_opening_passes_a_stalled_inventory_and_survives_its_stale_list` failed at its one-minute deadline. With independent inventory and scheduling locks the complete fixture passed in 1.27 seconds. It observed the webhook run while GitHub’s listing was held, preserved that run when the older empty snapshot returned, and cancelled it on the next fresh empty snapshot.
- `signed_outside_opening_queues_once_before_the_poll` passed in 1.60 seconds including fixture setup and job execution. Two concurrent signed deliveries queued one exact-head run, made zero PR get/list calls, published one pending status and started a worker. The existing CI sync found the same run afterward.
- Six focused integration tests passed together, including ordinary sync deduplication, fork exclusion, configuration repair and the existing queue, job, status and merge gate flow. The manifest test passed separately with the fake GitHub HTTP server. `scripts/dev quick`, `python3 scripts/decision_slots.py` and `git diff --check` also passed.
- Live webhook delivery to the incident repository requires activation of this instance’s existing GitHub App after rollout. The fixtures exercised both the sealed manifest key and an operator-configured signing key.
- Manual mutation removed the public webhook route. Both new end-to-end tests failed, observing HTTP 404 where the unsigned request expected 401 and the signed request expected 202. After restoring the route, nine focused composition tests passed together under `scripts/cargo nextest run -p yulius`. This run also covered sandbox removal, status-publication failure and merge-first CI. The manifest regression passed under `scripts/cargo nextest run -p yulius-github --features fake -E 'test(manifest_flow_builds_manifest_and_converts_code)'`.
- Review round one found three cases on the first commit. All three regression tests failed there. With an inventory status POST held, the signed opening reached its test deadline without publishing. With 30 admitted PRs, the 31st still queued. With 16 slow bodies, the route processed another request and returned 401.
- After moving publication outside the scheduling lock, the held-status fixture queued the PR while the POST was outstanding (1.67 seconds including setup). After sharing admission between inventory and openings, PR 31 waited through a reordered inventory and entered unchanged after a closure. After moving the permit before body extraction, the saturated route returned 429, all 16 readers timed out with 408 and a subsequent valid opening received 202. All five webhook tests passed together. The incident is tracked in [Yulius issue 1647](https://github.com/Quartio-Studio/yulius/issues/1647).
