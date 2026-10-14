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
session", and, during the task, a screenshot of "Onboarding studio … Follow onboarding" with "this
screen should not exsit for example".

## Goal

Pressing **Start onboarding** on the onboarding gate (SPEC-096) turns the gate into a loading state
at once, with nothing left to press, and when the server has started onboarding the page opens the
onboarding task, where its Claude session runs. The "Onboarding <name>" screen with **Follow
onboarding** no longer exists: whoever opens a sandbox while its onboarding runs lands on its task.

## Context

- The gate (`crates/yulius-server/web/assets/taskview.js`, `onboardingGate`, `approveOnboarding`)
  calls `POST /api/sandboxes/{sid}/onboarding/approve`. `singleFlight` drops a second press, but the
  button looked and stayed pressable for the whole request: no busy state, nothing disabled.
- The request is slow by nature: `start_onboarding` (`crates/yulius-app/src/onboarding.rs`) creates
  the onboarding task, starts its run (worktree, session spawn) and only then moves the sandbox to
  `preparing`. Task events reach the page while the request runs, so the pending gate was rebuilt
  with a link to the new task and the same live button: "a short lag when I can click it again".
- When the call returned, `renderBanner` showed the `preparing` gate: "Onboarding <name>" with
  **Follow onboarding** (SPEC-096 OBG-005), and the person had to click once more to reach the
  session: "I see a screen followup onboarding". The same screen showed to anyone opening the
  sandbox while onboarding ran (the person's screenshot), with variants for a task waiting for
  capacity (no action) and a stopped one (**Open onboarding**).
- Starting an ordinary task already works the way the person asks: the composer shows a working
  state and then opens the new task (`opened`).
- The task page already says each state of a task (running, waiting for capacity, stopped with
  Continue), so the gate's variants add nothing once the page opens the task.
- No server change: the approve call answers once the task has started; the page reads the task's
  number from the sandbox's task list (`origin: onboarding`), as the gate did.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBS-001 | Pressing Start onboarding replaces the gate's text and button with a loading state at once: the heading "Starting onboarding <name>", a line saying the agent's session is being started, and loading bars; the region is `aria-busy`. Nothing in it can be pressed, so the approval cannot be sent twice. | e2e with the approval held |
| OBS-002 | The loading state stays while the approval runs, even when the new task's events arrive meanwhile, and when the person leaves the page and comes back before it answers: the page shows neither the pending gate with a live button nor a Follow onboarding link. | e2e: events delivered while held; leave and return while held |
| OBS-003 | When the approval succeeds the page opens the onboarding task (`/s/<slug>/t/<number>`), without a further click. | e2e: URL after release |
| OBS-004 | When the approval fails, the loading state ends, the gate shows what it showed before with Start onboarding pressable and focused, and an error toast says onboarding could not start. | e2e with the approval failing |
| OBS-005 | Opening a sandbox's new-task page while its onboarding runs (`preparing`) opens its onboarding task in the page's place, whatever the task's state (running, waiting for capacity, stopped), replacing the history entry so Back does not bounce; until the sandbox's tasks are read it shows "Opening onboarding <name>" loading. There is no Follow onboarding or Open onboarding screen. When onboarding is started elsewhere (another person or tab) while the gate is open, the page opens the task too, with the toast "Onboarding started: opened #N.". New task (the shortcut, a New task button) for such a sandbox goes to its onboarding task directly, adding no history entry when already there. | e2e: three task states, Back, a sandbox preparing at app start (loading while its tasks are read), started elsewhere, New task |
| OBS-006 | Only when the page cannot open the task does it say so in words: the task list could not be read (onboarding runs, the task box appears when it lands) or the onboarding task is missing (the administrator's recovery path). The task box still comes back by itself when onboarding lands while such a page is open. | e2e (unchanged SPEC-096 cases, adapted) |
| OBS-007 | SPEC-096 (OBG-005), ADR-0090 (amending ADR-0076), `docs/changes.md` and `e2e/README.md` describe it. | review |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Start onboarding shows a loading state at once and ignores further presses
- [x] Start onboarding opens the onboarding task's session when it has started
- [x] Docs, changelog and review findings addressed
- [ ] Gates green, merged into dev on green CI

## Acceptance criteria

- AC-OBS-1: with the mock's approval held, right after the click the region "Starting onboarding
  website" is visible and `aria-busy="true"`, it has no button and no link, and the page has no
  "Start onboarding" button and no "Follow onboarding" link, also after the onboarding task's
  events arrive; axe finds nothing.
- AC-OBS-2: on release the URL is `#/s/website/t/1`.
- AC-OBS-3: with the approval failing, the gate "Onboard website first" is back, `aria-busy="false"`,
  Start onboarding focused, and the toast "Couldn’t start onboarding: …" shows.
- AC-OBS-4: with onboarding running, waiting for capacity or stopped, `#/s/website` becomes
  `#/s/website/t/1`, no Follow/Open onboarding link exists, and Back returns to the page before.
  The mock's `onboarding=preparing` opens `#/s/product/t/1`.
- AC-OBS-5: with the task list failing, "Onboarding product" says onboarding runs (not busy, not
  "missing"), centred, axe clean; finishing onboarding brings the focused task box back.
- `scripts/dev ci` and the e2e suite pass.

## Test plan

- e2e `e2e/tests/onboarding.e2e.ts` (mock mode), red first against the old `taskview.js` (5 of 8
  failing):
  - "Start onboarding loads at once and opens the onboarding session": hold the approval
    (`__yuliusMock.holdApprove`; the mock creates and starts the task first, then waits, then turns
    the sandbox `preparing`, as `start_onboarding` does).
  - "a Start onboarding that fails brings the gate back with its button".
  - "opening a sandbox while onboarding runs opens its task, never a Follow onboarding screen".
  - "a sandbox already preparing when the app opens goes to its onboarding task".
  - "a preparing sandbox whose tasks cannot be read says onboarding runs, and the task box comes
    back once it lands" (the former tasks-unknown case plus the landing part of the former Follow
    onboarding test).
- No Rust change; the server's approve route is covered by `crates/yulius/tests/projects.rs`.

## Decisions

- Navigate to the task after the approval returns or a `preparing` sandbox's task is known, not on
  the first task event while `pending`: only the approval (or the `preparing` state it leads to)
  says onboarding started; an event-driven jump would race a failure.
- A gate-wide loading state rather than a busy button only: the person asked for "the screen to
  become loading", and the gate's text would be stale while the server starts the session.
- Whether a start is in flight (per sandbox, for the tab) is part of `renderBanner`'s key; it keeps the loading state through the rebuilds that task
  events cause during the request.
- The person's screenshot widened the scope: the `preparing` screen goes for everyone, not only
  after a press here. Opening the task is a `replace` navigation so Back does not land on the page
  that redirects again; it is deferred one tick because the router mounts views synchronously and
  the composer may still be mounting.
- Kept in words: a task list that cannot be read and a missing onboarding task, where there is no
  task to open and the page must say why.
- Recorded as ADR-0090, amending ADR-0076's description of the gate's action.
- Review round 1 (fresh session) at 35f239f9: REQUEST CHANGES. Blocker: the loading line named the
  product in a string (repository check, red), now `BRAND.name`. Minors fixed: ADR-0076's Status
  and index entry and SPEC-001 IMP-010 name the amendment; a failed slow start no longer pulls the
  keyboard back when the person moved it (new e2e test, red first: focus was taken); `opening`
  resets after the navigation; nit: the "Opening onboarding" state while tasks load is asserted
  (held task list). Not changed: the `preparing` event arriving before the approval answers is
  handled (the composer is torn down, `destroyed` stops the rest) and the server's order is
  pinned by the mock, which follows `start_onboarding`.
- Review round 2 at 348353ea: APPROVE with 2 minors and 2 nits, all fixed test-first (each new
  e2e test red first): onboarding started elsewhere opens the task with a toast that outlives the
  page (the old live-region text was dropped with the page); New task on a preparing sandbox goes
  to its onboarding task directly instead of through the sandbox's page (history grew by one,
  the task view was rebuilt); a start in flight is kept per sandbox for the tab (a module-level
  set), so leaving and coming back still shows it loading; the Plan is ticked as steps finish.
