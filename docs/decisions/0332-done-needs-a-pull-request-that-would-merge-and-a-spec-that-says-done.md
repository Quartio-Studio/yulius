# 0332. Done needs a pull request that would merge and a spec that says done

## Status

Accepted. Specified in
[SPEC-491](../../.ai/specs/SPEC-491-a-task-whose-work-is-not-finished-never.md) (DON-001 to
DON-008); task #474. Amends SPEC-001 TSK-012, TSK-014 and GIT-001, SPEC-487 CHN-003,
[ADR-0328](0328-a-task-is-not-done-while-yulius-has-pull-request-news-for-it.md) ("final states
stay as they were") and [ADR-0325](0325-a-pull-request-delivers-the-finished-part-of-a-plan.md)
(what tells that a task with a complete plan still has work).

## Context

A task is Done when its turn ended, its plan is complete and its pull request is merged, or
open and tested (TSK-014). Chains, start rules, a supertask's wait, the session limit and the
task list ask that rule. On 14 November 2026 a person wrote "the task gets marked as done" about
task #465, which was still working: it read Done at about 18:11 UTC and again at 18:20, and the
tasks chained after it (#473, #474) started.

Part of that is closed on `dev` already: the instance ran `main` at `ec07d1331`, older than
ADR-0328, so an armed merge on a green pull request read Done. Two things `dev` still decided
wrongly:

- **A pull request that could not merge counted as tested.** `task_is_done` took an open pull
  request with checks `Success` or `None` and never asked whether it conflicts with its base. At
  18:20 pull request #1287 had a passed run at its head and conflicted with `dev`, which had
  moved four minutes before. "No check reported" also counted as "none required" there, while
  the Git overview has read it as pending under a profile that requires checks since #68: the
  sidebar said "In review" and the chain was released.
- **A plan complete so far counted as the whole work.** #465 adds a slice's step when the slice
  before it is merged, so between two slices its plan reads n/n (28/28 that day) with a merged
  pull request. ADR-0325 lets a plan hold later slices, but cannot make an agent list work it
  has not sliced yet. The spec of #465 said `status: in-progress` and named twelve findings
  still to fix.

## Decision

1. **Tested means the pull request would merge as it is.** One function reads it for chains,
   start rules, the Git overview and the counts (`PullSignal::from_wire`): the pull request is
   open, does not conflict with its base, and its checks passed at its head, or none are
   required. It conflicts when GitHub says it is not mergeable (a single pull request's answer;
   its lists say nothing) or when Yulius's own merge of its head with the integration branch,
   as its clone last fetched it, conflicts (`App::pull_conflicts`; the check the pull request
   watch tells the agent by, PRN-004). The API says so in `PullView.conflicts`.
2. **"None required" is the project's statement.** Without a Yulius CI run due at the head and
   with a delivery profile that requires no checks, a pull request without checks is tested.
   Where the profile requires checks, none reported is pending (`tested_checks_word`), for the
   chains as for the overview.
3. **A conflicting pull request is In review.** After a finished turn it reads "In review · PR
   #N · conflicts with `<base>`" and counts as working: the watch tells its agent, which merges
   the base in. The watch asks the head of the pull request's run, or, where no run exists (a
   project without Yulius CI), the branch's own head. Failing or errored checks keep their own
   reading, and an achieved goal stays Done.
4. **The spec's `status` is the task's record of whether its work is finished.** Yulius keeps
   it with the task whenever it reads the spec: at every turn's end, every few seconds of a
   live run and at every delivery call (`tasks.spec_status`, schema version 59). While it says
   `draft`, `approved` or `in-progress` the task has work left, however complete its plan
   reads: it is not Done by its plan and pull request, merged or tested, and idle it is the
   person's turn, as with open steps. `done` (which the validator accepts only with every Plan
   item ticked and Evidence) and `abandoned` say that nothing is left. A task without a spec
   declares its work by its steps alone.
5. **Every place asks the same.** `task_is_done` (chains, `#N done`, archive, delete),
   `task_is_done_locally` (the session limit, a supertask's wait) and `AttentionFacts::group`
   (Home, the tabs, the tools) read "work left" as open steps or such a spec; the web UI's
   `taskInfo` mirrors it from `TaskSummary.spec_unfinished`. A closed task whose spec says its
   work is not finished did not end Done: its chain needs a person.
6. **The agent is told.** With every step finished and a spec that does not say `done`, the
   step tools and `spec_check` say that the task is not Done and tasks waiting for it do not
   start, and how to go on: add the remaining work as steps, or set `status: done` with the
   Evidence.
7. **An achieved goal stays Done** (CHN-003), whatever the spec says: Claude Code's goal judge
   decided on the task's definition of done.

## Alternatives

- **Read the spec's prose** ("planned for later slices", an "open" row in a findings table).
  A heuristic on wording: every spec says it differently, and a sentence about history would
  hold a task for good.
- **A "final" flag on the plan, set by the agent.** The same statement as `status: done` a
  second time. Every existing task would lack it, and two records of one fact drift apart.
  The status is kept already: of 439 specs on `dev`, 437 say `done`, and the two that say
  `in-progress` are the two tasks that deliver slice by slice and are still working.
- **Done only with an achieved goal.** Rejected in ADR-0328: a goal judge that never says "met"
  would hold a chain for good, and prompt tasks have no goal.
- **Read the spec at every decision.** The counts ask for every idle task many times a minute,
  and a closed task has no worktree left. Yulius reads the spec at every turn's end anyway.
- **Ask GitHub for each pull request's mergeability in the overview.** One request per pull
  request per overview read, against the rate limit; and GitHub answers "not known yet" right
  after the base moved, which is when it matters. The clone's answer is what the agent is told.
- **A conflicting pull request needs you.** The watch tells its agent within 20 seconds, so
  every move of the integration branch would raise a false alarm. With Yulius CI, a pull request
  left that way for an hour is flagged as stalled (PRN-009).
- **A pull request behind its base is not tested either.** Its head passed and merges cleanly;
  bringing it up to date is part of the merge, which Yulius does when a merge is asked for. A
  task waiting only for a person's merge would flip to "In review" at every other merge.
- **Change the merge gate too** (`check_merge` lets a merge pass with no check reported). A
  project without any CI must be able to merge; only what reads as Done changes here.

## Consequences

- A task that delivers slice by slice is not Done between its slices: the tasks chained after
  it start when its spec says `done` (or its goal is achieved), not when a slice merges.
- A task that finished and forgot `status: done` reads "Your turn" instead of Done, and its
  chain waits; the task's title says why, its agent was told at the tick that completed the
  plan, and a person can start the chained task by hand. Goal tasks are Done through their
  goal.
- A task whose spec Yulius has not read since schema version 59 has no status kept, and is
  decided by its steps as before, until its next turn's end or delivery call.
- A conflict is known from the integration branch as Yulius's clone last fetched it: between a
  merge on GitHub and the next fetch a task may still read Done, as it may for any change
  GitHub has and Yulius has not read.
- In a project without Yulius CI the watch flags no stall (no run ever comes, PRN-009): a
  conflicting pull request whose agent was told and did nothing reads "In review · conflicts"
  until the agent or a person brings the branch up to date.
- Test fixtures that model a finished task write a spec that says `done`.
