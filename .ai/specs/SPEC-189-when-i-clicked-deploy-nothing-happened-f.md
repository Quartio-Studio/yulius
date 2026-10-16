---
yulius-task: 0b870c27-5ea0-4fbf-8e74-add86fbc9ed1
status: in-progress
---

# SPEC-189: a confirmed deploy answers at once and says what it is doing

Status: in progress
Date: 2026-10-16
Owner: Yulius task #174
Scope authorization: task #174 ("when I clicked deploy nothing happened for a while and I though it
broke, then it shown [the deploy] -- fix. It should be smooth and understable", two screenshots of
Yulius's own CD page). Fixes #585. Refines SPEC-134 CDT-005 (the CD page) and SPEC-151/152.

## Goal

From the moment a person confirms a deploy, the CD page shows that it is starting and what it is
doing, and the same dialog becomes the deploy's view when it has started.

## Context

Screenshots (2026-10-16): the person confirmed "Deploy production…" on Yulius's own CD page
(`[cd.prod]` with `promote = true`, no commands). Nothing changed on the page for a while; then the
Deploy dialog opened with the run already "Deployed", its stages reading confirmation 4 s, checks
12 s, promotion 10 s, deployment 0 s.

Root cause, traced in the code:

- `pages/cd.js` `deployProd`/`deployDev` close the confirmation, then `start()` awaits
  `POST /api/sandboxes/{sid}/cd/{env}/deploy`. Nothing on the page changes while it waits; only its
  answer opens the Deploy dialog (`showRun`) and a toast.
- The server answers that request only after `App::cd_start` → `cd_prepare`
  (`crates/yulius-app/src/ci/cd.rs`): refresh the sandbox clone, read the integration head from
  GitHub and fetch it into the CD mirror, read its `[cd.*]`, check the secrets, and for production
  with `promote` the promotion itself (a GitHub merge), then the new production head read and
  authorized again. The run record exists only after all of it, by design: a refused deploy
  records no run (SPEC-134 CDR-002, ADR-0109/0110). That is the "checks" span of the run, 12 s
  here, the promotion inside it.
- So the silence lasts as long as the checks and the promotion: seconds to a minute.

Why the tests missed it: the e2e mock answers the deploy at once. `cd.e2e.ts` holds the answer
(`holdCdDeploy`) only in the test about leaving the tab, which asserts the confirmation closes and
nothing about what the page shows meanwhile.

## Requirements

- CDS-001. Confirming a deploy opens the Deploy dialog at once, before the server answers, in a
  "Starting" state: the environment, its branch, who asked, what is being done now (for production
  with `promote`: checking, then promoting `<integration branch> at <promotes_from>` into
  `<branch>`; otherwise checking `<branch> at <head>`), how long it has taken so far (counting each
  second), and that its stages and output appear here as soon as it starts. Its content is polite
  live text, so a screen reader hears the state.
- CDS-002. When the server answers with the run, the same dialog becomes that run's view (state,
  stages, output, "In Pipeline"), without closing and reopening; the address names the run as
  before, and the run list refreshes.
- CDS-003. A refused deploy says why in that dialog: a "Not started" badge and the server's
  reason (which already says when nothing ran), instead of a toast behind it.
- CDS-004. Closing the starting dialog does not stop the deploy. When it starts, a toast says so
  with an "Open" action that opens its run; a refusal is a toast, as before. While the dialog is
  open it is the confirmation of the start, so no toast covers it. Leaving the CD tab
  meanwhile still opens nothing and names nothing in the address (SPEC-151 CCD-003).
- CDS-005. Nothing else changes: the confirmation (typed slug, `confirm_ms`, the commit it binds),
  the request, the server and what a refusal records.

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Write the fix spec and the regression test that fails
- [ ] The Deploy dialog opens at once and says what is starting
- [ ] A refused or left deploy reads right in the dialog
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green on the branch merged with current dev

## Acceptance criteria

- AC1 (CDS-001, CDS-002): in mock `studio`, with the deploy answer held, confirming "Deploy
  production…" shows the dialog "Deploy" with "Starting", "Promoting quartio at 9f3c2a1 into main"
  and an elapsed time that grows, with no WCAG violation; after the answer the same dialog element
  shows "Deploying", its output and the run in the address.
- AC2 (CDS-003): a dev deploy refused for a secret shows "Not started" and the reason in the
  dialog, and the dialog no longer says "Starting".
- AC3 (CDS-004): the starting dialog closed with Escape stays closed when the answer comes; a toast
  "Deploying production." with "Open" opens the run. Leaving the tab still draws nothing.

## Test plan

- e2e `cd.e2e.ts`: "a confirmed deploy opens its dialog at once and says what is starting" (AC1,
  AC3) and the existing dev refusal check extended (AC2). Both fail on `dev` at b62aaee3. The test
  "a tab left behind draws nothing when its answers come late" closes the now-open starting dialog
  before leaving the tab (the dialog is modal).
- Gates: `scripts/dev ci`, `scripts/dev security`, the whole e2e suite.

## Decisions

- **Show the start on the page; keep the request as it is.** Alternatives: (a) record the run
  before the checks and run them in the background, so the run's own view streams "checking",
  "promoting": the honest long-term shape, but a refusal would become a recorded failed run, the
  audit entry and the one-running-run lock would move ahead of the checks, and an agent's
  `cd_deploy` would change its answers: the trust contract of ADR-0109/0110 and SPEC-134
  CDR-002, far beyond a fix. (b) Keep the confirmation open with a busy button: still says nothing
  about what takes the time, and the confirmation is the wrong place to watch a deploy.
  (c) A toast "Starting…": easy to miss and has no room for the promotion. The chosen fix opens
  the dialog the person ends up in anyway, so the start and the run read as one thing.
- **What the dialog says comes from the card the person confirmed** (branch, head,
  `promotes_from`), the same facts the confirmation listed; the page does not guess the server's
  phase, so it names checks and promotion together ("Checking, then promoting …").
- **Elapsed seconds, not a fake progress bar.** The duration is unknown (a GitHub merge); a counter
  that moves proves the page is alive without promising an end.
