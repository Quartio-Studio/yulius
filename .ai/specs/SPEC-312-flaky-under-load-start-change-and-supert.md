---
yulius-task: c2a0e6b6-f66d-47b9-badd-c1861b283082
status: draft
---

# SPEC-312: Start-change, supertask, ownership and reply e2e checks wait on what they need (#506)

Status: draft
Date: 2026-10-25
Owner: Yulius task #298
Scope authorization: the issue-triage request (task #298). Fix GitHub issue #506 in one pull
request (`Fixes #506`). Create no new GitHub issue; comment progress and leftovers on #506.

## Goal

The e2e checks #506 lists pass at any host load, because each waits on the condition it needs,
with no retries and no longer first-load budgets (#354's open decision).

## Context

- **Symptom (#506).** Under host load 33-78, `start-change.e2e.ts` ("a chained task changes to
  When I decide…", `Starts:` chip click timed out), `supertask.e2e.ts` ("Supertask is a start
  choice…", `toHaveText` failed), `ownership.e2e.ts` (two cases, text timeouts) and, in a comment,
  `reply.e2e.ts` ("Reply on Needs you…", `toHaveText` after a `touchTask`) failed; they pass alone.
- **Already fixed elsewhere.** Editor presence: #523 (023b11c9, the `MONACO_LOAD_MS` wait). The
  ownership text the issue cites ("@anna-q owns this task:") was replaced by #572 (60d144b4).
- **Candidates (from triage at dev 9e1a2870).**
  1. Ambient activity: the mock changes studio tasks every 4 s tick after the events socket opens
     (#8 queued→starting at ~12 s, running at ~20 s; #12 touched every 16 s). The supertask's exact
     list of waited-for tasks depends on how far a slow test got.
  2. A `touchTask` before the page's events socket is open: the mock drops the event and the first
     connection does not resync.
  3. A task view detail read (`GET /api/tasks/{id}`) that answers after a newer `task.updated`
     event overwrites the view's task with the older state (`taskview.js` `loadDetail`).
  4. start-change clicks the chip right after the URL changes, before the chained task's view drew.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| FUL-001 | Each listed failure is reproduced (probe delays or a held CPU share) and its race named. | Evidence |
| FUL-002 | Each check waits on the condition it needs; no retries, no sleeps, no longer first-load budgets. | e2e files |

## Plan

- [ ] Spec SPEC-312 drafted with each test's race candidates
- [ ] Each failure reproduced with probe delays and its race named
- [ ] Supertask and reply checks independent of ambient activity
- [ ] Start-change waits for the chained task's view and the events socket
- [ ] Ownership touches wait for the events socket
- [ ] Task view keeps a newer event over an older detail read
- [ ] Files pass repeated runs under a held 0.2 CPU share
- [ ] Gate green, spec Evidence and changes entry written
- [ ] Independent review approves the final head
- [ ] Merged into dev with #506 commented

## Acceptance criteria

- Each race is forced by a probe and fails the unchanged check with the issue's signature; the
  changed check passes with the same probe.
- The changed files pass repeated runs under a held 0.2 CPU share, with no retries.

## Test plan

- Throwaway probes (not committed): delayed events socket, faster ambient ticks, delayed detail
  answers. Unchanged and changed checks under each.
- Repeated runs of the four files under a held 0.2 CPU share; `scripts/dev ci`; `scripts/dev security`.

## Decisions

- To be written as the causes are confirmed.
