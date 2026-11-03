# 0262. An e2e test's first look at a page has a first-load budget

## Status

Accepted. Specified in [SPEC-412](../../.ai/specs/SPEC-412-e2e-a-named-longer-wait-for-first-loads.md);
task #394, issues #354, #525 and #510. Amends
[ADR-0122](0122-the-e2e-server-keeps-up-with-a-page-under-load.md), which left this to a person's
decision, and sits beside [ADR-0251](0251-the-terminals-first-show-wait-outlasts-the-apps-reveal-limit.md)
(`TERMINAL_SHOW_MS`). Keeps `retries: 0` (SPEC-053).

## Context

After the e2e server stopped dropping connections (ADR-0122), about 1 run in 360 still failed at
a fifth of a CPU per thread, each time a default 5 s wait on a page's first load or read with no
product defect behind it (SPEC-148's proof): a heading, a banner, a region, the mock's install.
The first load of a page is real work on a starved runner (its modules, the mock's install, its
first in-page reads), and each test then gives it only an assertion's 5 s. Flaky required checks
block merges, so the person chose (2026-11-03, #354 option (a)) one named, longer wait for first
loads only, over accepting the residual rate (option (b)).

## Decision

- **One budget, `FIRST_LOAD_MS` (30 s, `e2e/tests/support.ts`), and one helper,
  `firstShown(locator, text?)`.** A test calls it for its first look at a page after `app.open` or
  a reload; it waits until the locator shows (with `text`, until it has that text). Everything
  after it keeps the runner's budgets: a slow step after the first load still fails.
- **`app.open` and `browser.reload` wait for the mock's install within `FIRST_LOAD_MS`**, since
  that install is part of every first load (`waitForTheMockOnEveryLoad`).
- **30 s**, the runner's action budget and `TERMINAL_SHOW_MS`: a page's first load is lighter
  than Monaco's parse (`MONACO_LOAD_MS`, 60 s).
- **Races stay fixed, not covered.** A failure whose cause is a race (the mock's ambient activity,
  a back-off a starved run stretches) is reproduced with probe delays and removed in the test;
  the budget is not stretched for it.
- **Regression tests** (`mock-ready.e2e.ts`) hold mock.js 6 s and hold a page's first read 6 s:
  both fail with a 5 s budget and pass with this one.

## Alternatives

- **Accept about 1 failure in 360 runs (option (b)).** Rejected by the person: a flaky required
  check blocks merges.
- **A longer `app.open` that waits for "the page is ready".** No single sign marks every route's
  first screen as done, and some tests hold reads on purpose (`hold=`); the test names what it
  waits for instead.
- **A longer default assertion budget (`expectTimeout`).** Every wait would get it, and every real
  failure would take six times as long to report; SPEC-053 removed blanket increases on purpose.
- **Retries.** Rejected again: `retries: 0` stays, a retry hides the wait instead of sizing it.

## Consequences

The tests that wait for their first screen with `firstShown` no longer fail for a starved first
load alone; a page that never shows still fails, after 30 s instead of 5 s. New tests should use
`firstShown` (or `terminalShown`, `editorShown`) for their first look at a page. A test whose first
look still uses a default wait keeps the residual rate until it is moved to the helper.
