---
yulius-task: 33e6f7c5-8caf-47ae-9153-d8042402120d
status: done
---

# SPEC-380: the Start-again test waits for the held GitHub read's answer

Status: done
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
- Product (issue #869): not a defect; test and mock only.
- Found on the way, and blocking every pull request's e2e job on 2026-11-02 (PR #1011's run
  failed on it too): `pipeline.e2e.ts` › "the daily chart's hover tip stays inside its chart on the
  first and the last day" fails at 1280 px on the last day, its tip 46 px past the plot's top.
  Cause: `placeTip` (`assets/stages.js`) always puts the tip above its point
  (`translate(-50%, calc(-100% - 8px))`), and the trend's scale (`niceMax`) rounds up from the
  highest p90, so that day's point sits about 20 px under the plot's top, less than the tip's
  height. The mock's daily values hash the date, so the highest day moves; today it is the last
  day, which the test hovers. SPEC-360 had left the flip out as "a case no timeline produces"; the
  daily trend produces it nearly every day, on whichever day is highest. No open issue tracked it,
  and this run may not open one, so it is fixed here, in its own commits.

## Requirements

- REQ-1: The mock counts the answered `GET /api/admin/github` reads (`githubAnswered`), bumped in
  the route after the `holdGithub` wait, as the answer is returned.
- REQ-2: After `releaseGithub` the test polls that counter until every asked read is answered (it
  goes up past its value before the release, the held read still unanswered then), then asserts
  Check now is absent and the form is shown.
- REQ-3: With the page's `epoch` guard removed, the test still fails.
- REQ-4: No product change for #869.
- REQ-5: A chart tip whose point is too near the plot's top for the tip above it goes below the
  point, the same 8 px away, when the plot has more room below the point than above it
  (`placeTip`, both the daily trend and the waterfall); otherwise it stays above.
- REQ-6: A regression test hovers the daily trend's highest day at 1280 and 390 px, checks the tip
  would not fit above, and requires it inside the plot and the window; it fails without REQ-5.
  An in-page unit test of `placeTip` (review round 2) covers a point with room above (above), one
  near a tall plot's top (below, inside) and one in a plot too short either way with more room
  above (above).

## Plan

- [x] Spec with the cause and the plan
- [x] Mock counts answered GitHub reads and the test waits on them, failing without the epoch guard
- [x] Daily chart's hover tip flips below a point near the chart's top (unblocks CI's e2e)
- [x] Gates green and independent review answered
- [x] Gate, e2e suite and review green on the final head

## Acceptance criteria

- The test waits on the answered-read counter after `releaseGithub`.
- It fails with the `epoch` guard removed (temporary edit, not committed) and passes with it,
  repeatedly.
- The onboarding-flow file and the whole e2e suite pass.
- The daily trend's tip stays inside its chart on its highest day; the new test fails without the
  flip and passes with it.

## Test plan

- Run the onboarding-flow file with the `epoch` guard removed (`if (mine !== epoch) return;`
  commented out) and see the test fail at Check now; restore it.
- Run the test 5 times and the file 3 times; then the full e2e suite and the project gate.
- Commit the highest-day test first and see it fail; add the flip; run `pipeline.e2e.ts` 3 times.

## Decisions

- Wait until the answered count equals the asked count, not just "went up": a read asked before
  `holdGithub` (the 5 s timer) could be answered after the release and make a bare "went up" pass
  before the held read's answer. Equal counts cover every asked read, the held one included. The
  test also checks the held read is unanswered before the release, so the poll must see it go up.
- The counter lives in the route, after the hold, rather than in `handleFetch`: only this route
  has the hold, and a counter of every answered GET would be a wider change than the test needs.
  The page still handles the answer after the mock returns it, but within the same few microtasks
  and tasks, before the poll's next browser round trip returns.
- The chart tip flips below its point rather than being clamped down to the plot's top: clamped,
  it would cover the point and the line it describes. The flip is decided in `placeTip` from the
  tip's measured height, so the waterfall's tip gets it too (its top row fits above today, so it
  does not change there). A mock that never puts a point near the top would only hide the case.
  It flips only when there is more room below (review round 2): a short waterfall's tall tip
  would otherwise trade a cut at the top for a bigger one at the bottom. `placeTip` is exported for
  its in-page unit test, as `dom.js` exports `menuMatches`.
- Fixed in this pull request, not a separate one: it fails every pull request's e2e job today, this
  run may not open an issue, and no other task was on it.

## Evidence

All on Linux, Node 24.21.0, Chromium from `npx e2e-web install chromium`, `E2E_TELEMETRY_DISABLED=1`.

- #869, guard removed (temporary edit, `// if (mine !== epoch) return;` in `pages/admin.js`, then
  `git checkout`): `npx e2e run tests/onboarding-flow.e2e.ts --grep 'Start again during the last
  read'` failed 3 of 3, each time at `expect.toHaveCount failed` (Check now came back), after the
  answered-read poll had passed.
- #869, with the guard: the test passed 5 of 5 alone; `tests/onboarding-flow.e2e.ts` 14/14 three
  times.
- Daily chart tip: the highest-day test failed before the flip (`1280 px, highest day: …"Mon, Oct
  12…","pastPlot":46`), committed red in 920bf234; after the flip `tests/pipeline.e2e.ts` 15/15
  three times. The in-page `placeTip` test failed before the room check (`expected true to be
  false`, the short-plot case), committed red; after it the file 16/16 twice and once more in the
  full run.
- Full e2e suite at 6c691b3a: `npx e2e run` 802 passed, 1 skipped (803), 71 files. Before the
  chart fix the same run at a9a7dbd9 failed only the daily chart test, as PR #1011's CI did.
- Project gate `scripts/dev ci` (after `python3 scripts/bootstrap.py`): exit 0 at 6cb1908c and at
  6c691b3a, nextest 2951 passed, 16 skipped.
- Security: `yulius security-scan` printed "Security scan disabled: Vidoc is turned off in Yulius,
  for this project or the whole instance; nothing was scanned." Disabled, not passed.
- Independent review in fresh sessions: round 1 approved 6cb1908c (comment wording nits fixed in
  a9a7dbd9); round 2 approved 56dbbf93 with a minor (flip without checking room below), fixed
  test-first; round 3 approved 6c691b3a. Left at the round limit: round 3's nit that the unit
  test's comment says "about 60 px" for a 70 px tip (no case depends on it).
- Left out, pre-existing, reported on #869 rather than filed (this run opens no issues): the
  timer window before `holdGithub` in the #869 test, and the Overview's "Tasks per day" tip
  (`pages/dashboard.js`), placed above its point without `placeTip`, which can rise past its plot
  the same way; SPEC-360's "only these two tips use `.chart-tip`" misses it.
