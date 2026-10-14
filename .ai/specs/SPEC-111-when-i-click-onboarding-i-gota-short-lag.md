---
yulius-task: e7d97052-fbf1-40c4-94be-926f5162f9e9
status: in-progress
---

# SPEC-111: Start onboarding loads at once and opens the onboarding session

Status: in-progress
Date: 2026-10-14
Owner: Yulius task #96
Scope authorization: the person's request (task #96), "When I click "Onboarding" i gota short lag
when I can click it again. If I wait for a moment, I see a screen "followup onboarding". Instead, I
want the screen to become loading for a moment and then come straight into onboarding claude
session".

## Goal

Pressing **Start onboarding** on the onboarding gate (SPEC-096) turns the gate into a loading state
at once, with nothing left to press, and when the server has started onboarding the page opens the
onboarding task, where its Claude session runs, instead of the "Onboarding <name>" message with
**Follow onboarding**.

## Context

- The gate (`crates/yulius-server/web/assets/taskview.js`, `onboardingGate`, `approveOnboarding`)
  calls `POST /api/sandboxes/{sid}/onboarding/approve`. `singleFlight` drops a second press, but the
  button looks and stays pressable for the whole request: no busy state, nothing disabled.
- The request is slow by nature: `start_onboarding` (`crates/yulius-app/src/onboarding.rs`) creates
  the onboarding task, starts its run (worktree, session spawn) and only then moves the sandbox to
  `preparing`. Task events reach the page while the request runs, so the pending gate is rebuilt
  with a link to the new task and the same live button: "a short lag when I can click it again".
- When the call returns, `renderBanner` shows the `preparing` gate: "Onboarding <name>" with
  **Follow onboarding** (SPEC-096 OBG-005), and the keyboard moves to that link. The person must
  click once more to reach the session: "I see a screen followup onboarding".
- Starting an ordinary task already works the way the person asks: the composer shows a working
  state (`working`, a busy start button) and then opens the new task (`opened`).
- ADR-0076 records the gate; this spec changes only what Start onboarding does after the press. The
  `preparing` gate stays for everyone who opens the sandbox while onboarding runs (another person,
  a reload, coming back), with Follow onboarding, its capacity, stopped and missing states.
- No server change: the approve call already answers once the task has started; the page reads the
  task's number from the sandbox's task list (`origin: onboarding`), as the gate does today.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBS-001 | Pressing Start onboarding replaces the gate's text and button with a loading state at once: the heading "Starting onboarding <name>", a line saying the agent's session is being started, and a loading placeholder announced to screen readers; the region is `aria-busy`. Nothing in it can be pressed, so the approval cannot be sent twice. | e2e with the approval held |
| OBS-002 | The loading state stays while the approval runs, even when task or sandbox events arrive meanwhile (the new task, the sandbox turning `preparing`): the page does not show the pending gate with a live button nor the Follow onboarding message. | e2e: events delivered while held |
| OBS-003 | When the approval succeeds the page opens the onboarding task (`/s/<slug>/t/<number>`), without a further click. | e2e: URL after release |
| OBS-004 | When the approval fails, the loading state ends, the gate shows what it showed before with Start onboarding pressable again, the keyboard is on it, and an error toast says onboarding could not start. | e2e with the approval failing |
| OBS-005 | When the onboarding task cannot be found after a successful approval (the task list could not be read), the page falls back to the `preparing` gate with its Follow or Open onboarding action focused, as before this change. | code path; e2e for the gate itself stays (SPEC-096) |
| OBS-006 | Opening the sandbox while onboarding runs still shows the `preparing` gate of SPEC-096 (Follow onboarding, capacity, stopped, missing). | existing e2e, adapted |
| OBS-007 | SPEC-096 (OBG-005), ADR-0076's successor ADR, `docs/changes.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] Start onboarding shows a loading state at once and ignores further presses
- [ ] Start onboarding opens the onboarding task's session when it has started
- [ ] Docs, changelog and review findings addressed
- [ ] Gates green, merged into dev on green CI

## Acceptance criteria

- AC-OBS-1: with the mock's approval held, right after the click the region "Starting onboarding
  website" is visible and `aria-busy="true"`, it has no button and no link, and the page has no
  "Start onboarding" button and no "Follow onboarding" link, also after the onboarding task's
  events arrive.
- AC-OBS-2: on release the URL is `#/s/website/t/1` and the task page shows that task.
- AC-OBS-3: with the approval failing (`failpost=`), the gate "Onboard website first" is back with
  Start onboarding focused and an error toast "Couldn’t start onboarding".
- AC-OBS-4: going back to `#/s/website` while it runs shows "Onboarding website" with Follow
  onboarding (SPEC-096 tests keep passing, adapted to the new start).
- `scripts/dev ci` and the e2e suite pass; axe finds nothing on the loading state.

## Test plan

- e2e `e2e/tests/onboarding.e2e.ts` (mock mode), red first:
  - new: "Start onboarding loads at once and opens the onboarding session" — hold the approval
    (`__yuliusMock.holdApprove`, the mock creates and starts the task first, then waits, then turns
    the sandbox `preparing`, as `start_onboarding` does), check the loading state and its stability
    under events, release, check the URL.
  - new: the failing approval brings the gate back with the button focused.
  - adapted: "Start onboarding runs it…" now lands on the task, then goes back to the sandbox to
    check the `preparing` gate and the task box coming back.
- The mock's approve route gains the hold, mirroring the server's order (task first, `preparing`
  last).
- No Rust change; the server's approve route is covered by `crates/yulius/tests/projects.rs`.

## Decisions

- Navigate to the task after the approval returns, not on the first task event: the approval is
  the one answer that says onboarding started; an event-driven jump would race a failure. The page
  reads the task number from the reloaded task list, as the gate already does.
- A gate-wide loading state rather than a busy button only: the person asked for "the screen to
  become loading", and the gate's text (the plan, "Start onboarding") would be stale while the
  server starts the session.
- A `starting` flag in `renderBanner`'s key keeps the loading state through the rebuilds that task
  and sandbox events cause during the request; it is the cause of the "Follow onboarding" flash.
- Keep the `preparing` gate and Follow onboarding for every other visit: others still need it.
- Recorded as an ADR that amends ADR-0076's description of the gate's action.
