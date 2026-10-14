---
yulius-task: 921b91de-8418-43bf-bcdf-61c61b43575d
status: done
---

# SPEC-104: pull request chips show at once after a reload

Status: done
Date: 2026-10-14
Owner: task #89
Scope authorization: the person's request (task #89), "after reloading prs are not visible at the
beggining", with two screenshots of the sidebar's Working group: right after a reload no row has a
pull request chip; moments later #400, #405 and #408 appear (the rows' times are unchanged, so it
is seconds, not a minute).

## Goal

After a reload the sidebar (and every view that reads `store.pullsOf()`) shows the pull requests it
last saw for the sandbox at once, and the fresh Git overview read replaces them when it answers.

## Context

- **Symptom.** Reload any sandbox page: the sidebar rows have no `#N` chip until the first
  `GET /api/sandboxes/{id}/git` answers; then they appear, and rows can change group (a tested pull
  request is Done, SPEC #35) and their status mark.
- **Root cause.** `store.js` keeps the pull requests in memory only (`state.pulls`, filled by
  `loadPulls`), so every page load starts with none. The only source is the Git overview
  (`App::git_overview`, `yulius-app/src/delivery.rs`), which lists the pull requests on GitHub,
  reads the checks of each (or the Yulius CI run) and runs three Git commands per task with a
  branch, one after the other: seconds on a real sandbox. Until it answers, `pullsOf()` is empty.
- **Why tests missed it.** The mock answers the overview at once, and no test looked at the rows
  while that read was pending.
- **What else the cause touches.** Everything that reads `pullsOf()` before the first overview:
  the sidebar's groups and marks (`taskInfo`), its quick Archive (`taskActions`), the Tasks page's
  pull request column, the task header's chip and the notification grouping. They all get the
  remembered value with this fix.
- **Prior art.** `drafts.js` keeps per-user, per-sandbox data in `localStorage`
  (`yulius.draft.<user id>.<sandbox id>`) and sign-out forgets it (`app.js` `signOut`).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PRC-001 | Every successful overview read stores the sandbox's task → pull request pairs in this browser under `yulius.pulls.<user id>.<sandbox id>`. | `pulls.e2e.ts` "the fresh read replaces …" |
| PRC-002 | Until the first overview read of a page load answers, `pullsOf(sandbox)` returns the stored pairs of the signed-in user for a sandbox a view watches (its read is on its way), so the sidebar shows them at once. A sandbox no view watches (a notification about another sandbox) gets nothing stored, as before: no read would replace it. | "after a reload the sidebar shows the pull requests at once …", "a sandbox no view watches …" |
| PRC-003 | The fresh read replaces the stored pairs, in memory and in storage. When the page's first read of the sandbox fails, the stored pairs are dropped from view (shown already or not) instead of passing for current; a later successful read fills them. | "the fresh read replaces …", "a failed first read drops …" |
| PRC-004 | A stored value of the wrong shape (not a list, or rows without a task id, a whole number or a state), or unavailable storage, is ignored: the rows wait for the overview as before. | "a stored value that is … is ignored …" |
| PRC-005 | Signing out or an expired session (noticed by a request, or at page load) removes every stored `yulius.pulls.` entry and the ones in memory: pull request titles are the user's data. A read answering after that finds no user and writes nothing to storage (it can still fill memory, as before this change). | "signing out removes …", "a session that expires removes …", "a session found expired when the page loads …" |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] Fix the cause until the regression test and every gate pass
- [x] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- In the mock, open `/s/studio`, then reload with `hold=/api/sandboxes/sb_studio/git`: the sidebar
  shows the same chips as before the reload while the overview is held, and still after release.
- A stored pull request number that the overview no longer says shows until the read answers and
  is gone after it, also from storage.
- A malformed stored value shows no chips and no page error; after the read the chips show.
- With a failing pull request remembered for studio's #12, a page load of `/s/product` and a
  change to #12 bring a notification that does not mention it.
- With the overview failing (`fail=/api/sandboxes/sb_studio/git`), a remembered chip is not shown.
- After sign-out, and after the session expires (during use or found at page load), no
  `yulius.pulls.` key is left.

## Test plan

- `e2e/tests/pulls.e2e.ts` (mock, Chromium), the cases above. The first fails before the fix
  (no chips while the overview is held); after the independent review, the unwatched sandbox and
  the expired session cases were added and failed before their fix; after the second, the failed
  first read and the expiry found at load (both failed before their fix), and the wrong-shape rows
  now name real tasks (the case fails with the row filter removed).
- Full gate: `scripts/dev ci` and the whole e2e suite.

## Decisions

- **Remember the last read in the browser (chosen)** over making the server overview faster or
  caching it on the server. A server cache (the overview is already summarised in
  `ObservedPulls`) is empty after every restart, and every deploy restarts it; making the overview
  concurrent shortens the wait but does not remove it. The browser copy shows chips in the first
  frame for reloads and new tabs alike, with no server change. A remembered chip can be stale for
  the seconds the read takes; the read always runs and replaces it. Reversible: one store
  function. Small and local, so no ADR.
- **`localStorage`, keyed by user and sandbox, forgotten at sign-out and session expiry**, over
  `sessionStorage`, which would not help a new tab. Drafts survive an expiry (they are the
  person's unsent words); the pull requests are only a head start, so they go with the session.
- **Seed only a watched sandbox** (review finding): a remembered value is only safe while a read
  is on its way to replace it. Another tab that signs out while this one writes a read back is
  covered when this tab's next request finds the session gone (it tears down and forgets).
- **No age limit on a remembered value**: a watched sandbox's read always runs at once and
  replaces it, so its age only matters for those seconds; when that read fails the remembered
  value is dropped (second review), so an old one never stays on screen.
- **Stop after three review rounds.** Round 3 approved 6af0517d. Its remaining minor, that no test
  covers "a failed later read keeps the real chips", concerns behaviour that predates this change,
  and a reliable test needs a mock signal for failed reads: tracked in #415. Its nit about memory
  was fixed in the wording (a read answering after sign-out can still fill memory, as before).

## Evidence

- Regression test first: `a93e7e56` alone, `npx e2e run tests/pulls.e2e.ts`: "after a reload the
  sidebar shows the pull requests at once …" fails (no chips while the overview is held); the fix
  `3d9288a4` makes it pass. Review rounds: tests `97454376` and `ab44a79b` failed before their
  fixes `c6e95d12` and `6af0517d` (unwatched sandbox, expiry during use and at load, failed first
  read); the wrong-shape case fails with the `isPullRow` filter removed (checked by hand).
- `npx e2e run tests/pulls.e2e.ts`: 9 passed.
- `scripts/dev ci` (repository check, unit tests, cargo check, fmt, clippy `-D warnings`, nextest,
  docs) on the merged head: exit 0, nextest 1533 passed, 16 skipped.
- Whole e2e suite (`npx e2e run`) on 6af0517d: 43 files passed, 1 skipped; 304 tests passed,
  1 skipped.
- `python3 scripts/check_repository.py` on 6af0517d: passed.
- Independent reviews in fresh sessions: 3d9288a4 approved (3 minors, fixed), 583e8b6d approved
  (3 minors, fixed), 6af0517d approved (1 minor tracked in #415, 1 nit fixed in wording).
- Not testable here: the real server's overview latency on the reported sandbox (no access to the
  deployed instance); the mock's hold stands in for it.
