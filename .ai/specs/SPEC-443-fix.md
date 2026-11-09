---
yulius-task: abfcc1bf-8f36-4968-a785-3d973df436d8
status: done
---

# SPEC-443: A failed update check says why and is retried within minutes

Status: done
Date: 2026-11-09
Owner: Yulius task #425, issue #1124
Scope authorization: the person's request (task #425), a screenshot of Settings → Instance →
Updates reading "GitHub could not be reached." on a server running `ec56257`, 10 minutes after a
restart: "fix".

## Goal

When an update check fails, the person reads what actually went wrong, and the warning clears by
itself once GitHub answers again.

## Context

- **Symptom.** On 2026-11-08 at 11:42 UTC the Updates card showed "GitHub could not be reached."
  while `main` (`7923acbe`) was 7 commits ahead of the build, so no update was offered.
- **What happened.** GitHub's status history has "Increase in timeouts for some unauthenticated
  users" on 2026-11-08 from 10:36 to 12:32 UTC (API Requests degraded). The client works: run
  against api.github.com on 2026-11-09 it read `main` in about 1 s and compared `ec56257` →
  Behind, 7 commits.
- **Root cause in Yulius.** `GitHubReleases::get` (`crates/yulius/src/wiring/release.rs`) maps three
  different failures to `SourceError::Unavailable` ("GitHub could not be reached"): no connection,
  a request past its 15 s deadline, and any unexpected HTTP status (a GitHub 5xx). So a GitHub
  outage reads like a broken network on the host. And `App::update_round` schedules the next
  automatic check an hour after a failed one, so the warning outlives the outage until someone
  reopens the page or clicks Check now (SPEC-149's page-open check only runs while someone looks).
- **Why the tests missed it.** `refusals_and_rate_limits_are_typed` asserted that a 500 *is*
  `Unavailable`; no test drove `update_round` with an automatic interval and a failing source.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| UCF-001 | A request that runs past its deadline is `SourceError::TimedOut`, shown as "GitHub did not answer in time"; an unexpected HTTP status is `SourceError::Status(n)`, shown as "GitHub answered with an error (HTTP n)"; only a request that got no answer at all stays `Unavailable` ("GitHub could not be reached"). | `release.rs` `failures_say_whether_github_answered` |
| UCF-002 | After any failed check (automatic, "Check now" or the page's own check on opening), the next automatic one runs at most `CHECK_RETRY_AFTER` (5 minutes) later, never later than the configured interval; after a successful one, or a rate limit (retrying into it only fails again), the configured interval later (hourly). | `self_update.rs` `a_failed_automatic_check_is_retried_within_minutes`, `a_failed_check_now_brings_the_automatic_check_closer`, `a_rate_limit_waits_the_interval_and_a_short_interval_stays` |
| UCF-003 | A check whose failure starts, changes or ends is published (`update.updated`) even when the offered update stays the same, so an open page shows and clears the warning without a reload. | `self_update.rs` `a_failed_and_a_recovered_check_are_published` |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- A compare that hangs past the client's deadline reads "GitHub did not answer in time."; a 502
  reads "GitHub answered with an error (HTTP 502)."; a connection closed without an answer reads
  "GitHub could not be reached.".
- With hourly checks, a failed check (automatic or "Check now") is followed by another 5 minutes
  later; once one succeeds, or after a rate limit, the next is an hour later.
- An open Instance page shows a failure and its end as they happen.

## Test plan

- Regression (unit, `wiring/release.rs`): a local server that never answers (with a short
  deadline) and one answering 502 give `TimedOut` and `Status(502)` (fails today: both are
  `Unavailable`); a body that stalls is `TimedOut` and a connection closed without an answer stays
  `Unavailable`.
- Regression (integration, `tests/self_update.rs`): an instance with hourly checks and a fake
  source that fails: `update_round` checks, the fake clock moves 5 minutes, `update_round` checks
  again (fails today: the next check is an hour away); then a success moves the next check to an
  hour later. Review round 1 added: a failed "Check now" brings the automatic check to 5 minutes
  away, a failure and its recovery are published with no offer change, a rate limit keeps the
  hour, and a 2-minute interval stays 2 minutes after a failure.
- What else the cause touches: only the update check reads `SourceError`; the retry only shortens
  the wait after a failure, so a healthy instance makes no extra GitHub calls. A lasting failure
  costs 12 checks an hour of up to 3 requests each; an anonymous instance (60 requests an hour) can
  afford that, and a rate limit itself is never retried early.
- Gates: `scripts/dev check`, `scripts/dev lint`, `scripts/dev test`, `scripts/dev security`.

## Decisions

- Retry after every failure but a rate limit: a refused credential or an unreadable answer can
  also be fixed on GitHub's side and a check every 5 minutes is cheap, while retrying into a rate
  limit only fails again (GitHub asks clients to wait it out). No backoff beyond the fixed
  5 minutes.
- The retry belongs to every failed check, not only the automatic ones: a failed "Check now" or
  page-open check replaces the offer for every superadmin, so it must clear by itself as well
  (review round 1). The automatic loop sets its next check before checking; a failure only ever
  brings it closer (`fetch_min`).
- A change of the failure is published like a change of the offer: clients read the view again
  only on `update.updated`, and an up-to-date server's offer never changes (review round 1).
- No UI change: the banner shows the server's message, which now carries the reason.
- A 3xx (redirects are off) reads "GitHub answered with an error (HTTP 301)": GitHub moves a
  renamed repository that way, which is still a reason the check failed; left as is (review nit).

## Evidence

- Root cause: GitHub's incident history (`githubstatus.com/history.atom`) has "Increase in
  timeouts for some unauthenticated users", 2026-11-08 10:36–12:32 UTC; the screenshot is from
  11:42 UTC. The unchanged client, run once against api.github.com from the sandbox (a temporary
  ignored test, not committed): `main` read in 1.03 s, `ec56257` compared → Behind, 7 commits.
- Regression first (`0c9e3486`, tests only): `failures_say_whether_github_answered` failed
  (`Unavailable` vs `TimedOut`), `a_failed_automatic_check_is_retried_within_minutes` failed
  (1 compare vs 2). With the fix (`bad352f8`) both passed.
- Review round 1 (fresh session, request changes on `bad352f8`): 2 major, 3 minor, 3 nits, all own
  code. Tests first (`96aadd8d`): `a_failed_check_now_brings_the_automatic_check_closer` (2 vs 3),
  `a_failed_and_a_recovered_check_are_published` (0 vs 1 events),
  `a_rate_limit_waits_the_interval_and_a_short_interval_stays` (2 vs 1) failed; with `daf44c3b`
  all passed. Nit 7 (3xx) kept, see Decisions.
- Review round 2 approved `daf44c3b`; its nits: the acceptance criterion now names the tested
  closed connection; the stalled-body case is kept as is (with one deadline it cannot tell a
  header from a body timeout, and both are `TimedOut`).
- `scripts/dev ci` (check, lint, deps, macOS, tests, docs) at `bad352f8`: exit 0, 3252 tests
  passed, 16 skipped; at `daf44c3b`: exit 0, 3255 passed, 16 skipped.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius"; nothing was
  scanned (disabled, not passed).
- Not tested: a real GitHub outage against the deployed server (no host access from the sandbox;
  the server's logs now name the failure and the credential, `via`).
