---
yulius-task: 7ec87fac-9cd8-4d2b-a9a0-7ef749f205d0
status: in-progress
---

# SPEC-108: a pull request change reaches the sidebar when it happens

Status: in-progress
Date: 2026-10-14
Owner: task #93
Scope authorization: the person's request (task #93), "we got wrong status again, investigate
status even deeper than before", with a screenshot of the sidebar's Done group taken 2026-10-13
13:05:24 (CEST, 11:05:24 UTC): task #56's row "Fix #243 #320 #322 #2…", 3m, sits in Done with its
pull request chip `#340` and a yellow clock ("checks running"), while the rows below show merged
chips.

## Goal

When Yulius learns that a sandbox's pull requests changed (one opened, merged, closed or
reconciled with GitHub, or a Yulius CI run at a head started or ended), every browser watching that
sandbox reads them again at once, so a task's row, header and counters never combine a live task
state with a pull request read from minutes before.

## Context

- **What the screenshot shows, from task #56's transcript.** At 10:49:52 UTC the agent armed the
  merge of PR #340 (checks pending) and waited in `yulius wait`. Yulius CI passed and Yulius merged
  the pull request at 11:00:56 (`980059c`, committer date 13:00:56+02:00). `yulius wait` returned
  at 11:00:59, the agent closed the issues and ended its turn at 11:02:03 with its goal achieved:
  the task was Done (the row's "3m"). At 11:05:24, 4.5 minutes after the merge, the sidebar still
  showed #340 open with checks running. The row's group was right; its pull request was a read from
  before 11:00:56, while CI was still running.
- **Root cause.** The task's state reaches the browser live (`task.updated` on the event stream),
  but its pull request comes only from the sandbox's Git overview (`GET /api/sandboxes/{id}/git`),
  which the browser polls every 60 s, never while the tab is hidden, and at most every 15 s when it
  is shown again (`store.js` `watchPulls`, PERF-12). The server never says that a pull request
  changed, although it knows: it merges them itself (`dlv_merge`, armed merges, merge trains), it
  opens them, it reconciles them with GitHub (`dlv_observe`, `reconcile_deliveries`) and it runs
  their checks (Yulius CI). So a task's row combines a live state with a pull request read up to
  a minute old in a visible tab, and of any age in a tab that was hidden (Chrome hides a covered
  window too) until the next read answers, seconds after the tab is shown again.
- **Where else the stale read is wrong, beyond the chip.** `taskInfo` (`status.js`) and its server
  twin `AttentionFacts::group` decide a finished task's group from that read:
  - an achieved goal with a stale "checks failing" read reads **Needs you · PR #N checks failing**
    (the failing-checks branch comes before the achieved-goal branch), although the checks were
    fixed and the pull request merged;
  - a finished turn with a complete plan and a stale "checks running" read reads **In review**
    (Working) after the merge, and a stale "checks passed" read of an earlier head reads **Done**
    while the checks of the new head run;
  - the server's Home and tab counts (`ObservedPulls`) are recorded from the same browser reads.
- **A read in flight.** `loadPulls` lets reads overlap and the last to answer wins. A read that
  started before a change and answers after it shows the old state until the next poll.
- **Prior work.** SPEC-032 and SPEC-085 (status truth) fixed how a task's own state is read from
  Claude Code's records; SPEC-104 shows the last read at once after a reload; #294 (SPEC-067) made
  the task header follow a late read. None made the read follow a change.
- **Why tests missed it.** The mock changes pull requests only through its own routes, and no test
  changed a pull request while a view watched it.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| PRL-001 | The event stream carries `pulls.updated { sandbox_id }`, addressed to the sandbox (only people who may view it receive it). It says the sandbox's pull requests may have changed; it carries no pull request data. | `delivery::pull_request_changes_are_announced_to_the_sandbox`, `local_ci::a_run_announces_its_pull_requests` (the envelope's sandbox) |
| PRL-002 | The server publishes it whenever a task's delivery stage moves (a pull request opened, merged, closed, reconciled with GitHub, an environment or promotion step), by any path: a person, an agent's tool, an armed merge, a merge train, the reconcile loop, a playground merge. A read that finds the stage unchanged publishes nothing. | `delivery::pull_request_changes_are_announced_to_the_sandbox` |
| PRL-003 | The server publishes it whenever a Yulius CI run of the sandbox is queued, starts or ends (its commit status is published), except a merge train's run, which is no pull request's head. | `local_ci::a_run_announces_its_pull_requests` |
| PRL-004 | A browser that watches the sandbox and is in view reads its pull requests again on the event: at once, or 3 s after the start of its last read when that was more recent (one timer collects the events meanwhile), so a busy sandbox costs at most one read per 3 s per tab instead of reads back to back. | `pulls.e2e.ts` "a merge Yulius announces reaches the row at once", "announced reads of a sandbox keep their gap" |
| PRL-005 | A read never answers with older news than the newest change it was told about: reads of a sandbox do not overlap, also when a view stops watching it and another starts during a read, and events during a read make one more read after it, including one announced while no view watched it (the view that watches it again during that read gets it). | `pulls.e2e.ts` "an announcement during a read is read again after it", "watching a sandbox again during its read starts no second read", "a change announced while nobody watched reaches the view that watches again" |
| PRL-006 | A hidden tab reads nothing on the event (PERF-12 stays), and reads at once when shown again, whatever the 15 s spacing, if an event came while it was hidden. `resync` (lost events, a restart) still reads at once, hidden or not: the server's counts start from such a read (#147). | `pulls.e2e.ts` "a hidden tab reads nothing on an announcement and catches up when shown" |
| PRL-007 | The Git overview reads a pull request that a task's delivery record says is merged (its stage past the merge, same number) as merged, even while GitHub's list of open pull requests, read just after the merge, still has it. | `delivery::a_merged_pull_request_github_still_lists_as_open_reads_merged` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The server announces a delivery stage change of a sandbox's pull request
- [ ] The server announces a Yulius CI run change of the sandbox
- [ ] The browser reads the pull requests again on the announcement, after a read in flight
- [ ] A hidden tab catches up on announcements when it is shown again
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- In the mock, `/s/studio` shows task #13 (achieved goal) in Done with PR #520 "checks running".
  The mock merges #520 and announces it: the chip reads merged within 3 s, not after the 60 s poll.
- With the Git overview held, an announcement made during the read brings a second read after it,
  which shows the merged pull request.
- With the tab hidden, an announcement makes no read; showing the tab reads at once even 1 s after
  the last read.
- In the integration tests, opening a pull request, merging it and a merge on GitHub that the
  reconcile loop finds each publish `pulls.updated` for the sandbox once; a status read that
  changes nothing publishes nothing; each Yulius CI run publishes it when queued, when it starts
  and when it ends (exactly, even with GitHub refusing the commit status); a pull request
  recorded as merged reads merged while GitHub still lists it open.

## Test plan

- Rust: `delivery.rs` integration test subscribing to the event bus around open, merge, reconcile
  and an unchanged observe; `local_ci.rs` test around a sync and a settled run (with GitHub
  refusing the commit status). Each fails without the publishing call.
- e2e (mock, Chromium): `pulls.e2e.ts` cases for PRL-004..006, each failing on the old `store.js`.
  The mock announces the pull request changes it makes (open, merge, checks passing), gets
  `announcePulls(sandbox)`, `changePull(sandbox, number, changes)`, per-read holds
  (`holdGitReads`, `releaseGit()` newest first; a held read answers what it saw when it started)
  and `gitRequests`, counted as the page asks, the sync point that lets a test prove a read did
  not happen.
- Full gate: `scripts/dev ci` and the whole e2e suite.

## Decisions

- **An event that says "read again", not the pull request itself.** The overview is the one place
  that combines GitHub's list, Yulius CI and the delivery records per task, for the person's own
  GitHub access. Pushing pull request data would need a second, per-viewer projection of it.
  The event costs one overview read per watching browser per change, which is what a poll costs,
  and bursts collapse into one read. ADR-0087.
- **Published at the two places that change pull request facts**: the delivery stage write
  (`dlv_transition`, wrapping the store's compare-and-set; every stage write of `yulius-app` goes
  through it: opening, merging, reconciling, environments and promotion, playground merges) and
  the CI commit status publication (`ci_publish`, on a run's queueing, start and end; the status
  retry loop announces nothing, as it changes no run). The tests cover opening, merging and
  reconciling; the other stage writes share the wrapper.
- **At most one announced read per 3 s per tab.** A push brings several events (the run queued,
  started, ended, a superseded run cancelled), and each read costs a GitHub list and Git commands
  per task branch. A 3 s gap keeps a busy sandbox from reading back to back and still shows a
  change within seconds. The guard that keeps one timer per watch (`entry.soon`) has no test of
  its own: without it, a burst schedules several timers whose reads collapse through the in-flight
  rule into at most one extra read.
- **Yulius's own merge record wins over GitHub's list** of open pull requests (PRL-007): GitHub's
  lists are eventually consistent, and the read an announced merge brings comes right after it.
- **The polling stays** (60 s while in view) for changes Yulius does not see: a push to GitHub
  without a Yulius CI run, a merge on GitHub before the reconcile loop's next pass, another CI.
- **No change to the rule of `taskInfo` and `AttentionFacts::group`.** Its order is right for
  current facts (SPEC-085); the defect was stale facts.

## Evidence

To be filled when done.
