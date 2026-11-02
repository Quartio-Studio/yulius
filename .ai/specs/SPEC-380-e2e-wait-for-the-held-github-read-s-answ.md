---
yulius-task: 33e6f7c5-8caf-47ae-9153-d8042402120d
status: in-progress
---

# SPEC-380: the Start-again test waits for the held GitHub read's answer

Status: in-progress
Date: 2026-11-02
Owner: Yulius task #362
Scope authorization: task #362, Fixes #869 (a nit left by task #269's review, SPEC-283).

## Goal

`e2e/tests/onboarding-flow.e2e.ts` › "Start again during the last read keeps the form, and Check
now is off while it reads" checks that the released read's answer left the form alone only once
that answer has come, so it cannot pass before the answer it is about.

## Context

- After `releaseGithub` the test read `githubReads` (the mock's `asked['/api/admin/github']`,
  bumped by `handleFetch` as a GET is asked) and polled it to stay the same. The held read was
  already counted when it was asked, so the poll matched at once, and `toHaveCount(0)` on Check now
  could run before the read's answer reached the page.
- The mock's `GET /api/admin/github` route (`crates/yulius-server/web/assets/mock.js`) waits on
  `db.holdGithub` and counted no answers, so the test had nothing to wait on.
- The guard the test proves: `epoch` in `crates/yulius-server/web/assets/pages/admin.js`
  (`check` takes `mine = epoch` before the read and returns when Start again bumped it meanwhile).
  Without it, the released read, which noticed the 15-minute limit, puts the Check now card back.
- Today the test passes and fails without the guard only because the evaluate round trips are
  slower than the answer; nothing says so.
- Product: not a defect; test and mock only.

## Requirements

- REQ-1: The mock counts the answered `GET /api/admin/github` reads (`githubAnswered`), bumped in
  the route after the `holdGithub` wait, as the answer is returned.
- REQ-2: After `releaseGithub` the test polls that counter until every asked read is answered (it
  goes up past its value before the release, the held read still unanswered then), then asserts
  Check now is absent and the form is shown.
- REQ-3: With the page's `epoch` guard removed, the test still fails.
- REQ-4: No product change.

## Plan

- [ ] Spec with the cause and the plan
- [ ] Mock counts answered GitHub reads and the test waits on them, failing without the epoch guard
- [ ] Gates green and independent review answered
- [ ] Pull request merged into dev on green CI

## Acceptance criteria

- The test waits on the answered-read counter after `releaseGithub`.
- It fails with the `epoch` guard removed (temporary edit, not committed) and passes with it,
  repeatedly.
- The onboarding-flow file and the whole e2e suite pass.

## Test plan

- Run the onboarding-flow file with the `epoch` guard removed (`if (mine !== epoch) return;`
  commented out) and see the test fail at Check now; restore it.
- Run the test 5 times and the file 3 times; then the full e2e suite and the project gate.

## Decisions

- Wait until the answered count equals the asked count, not just "went up": a read asked before
  `holdGithub` (the 5 s timer) could be answered after the release and make a bare "went up" pass
  before the held read's answer. Equal counts cover every asked read, the held one included. The
  test also checks the held read is unanswered before the release, so the poll must see it go up.
- The counter lives in the route, after the hold, rather than in `handleFetch`: only this route
  has the hold, and a counter of every answered GET would be a wider change than the test needs.
  The page still handles the answer after the mock returns it, but within the same few microtasks
  and tasks, before the poll's next browser round trip returns.
