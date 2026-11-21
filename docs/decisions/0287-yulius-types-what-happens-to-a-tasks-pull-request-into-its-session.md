# 0287. Yulius types what happens to a task's pull request into its session

## Status

Accepted. Specified in [SPEC-444](../../.ai/specs/SPEC-444-tasks-always-notice-what-happens-to-thei.md);
task #426. Amends SPEC-028 MSR-004 ("nothing is typed into an idle session") for a task with an open
pull request; builds on [ADR-0285](0285-armed-pull-requests-merge-through-a-queue-that-never-restarts-a-run.md).

## Context

A task heard about its pull request only through `yulius wait`, a background shell it had to start
and keep alive: it ended after 90 minutes while runs were still queued, and with a merge armed it
did not end on a red run at its own head. Claude Code's goal check-ins pause "until your next
message" after the first 30 minutes. In quartio-v2 on 2026-11-08, tasks sat for 10 minutes to an
hour after a red run, a conflict or a green pull request that was not merging, until a person's
session nudged them; some re-armed or merged `dev` inside a passing merge train, dissolving it, or
planned to merge by hand.

## Decision

- A server-side watch (every 20 s) looks at each running task with an open pull request, from
  Yulius's own records and clone, and types what needs the task into its session as one message
  with the next step: a red run at its head or of a merge candidate holding it (jobs, failing
  tests, where to read the log), a conflict with the integration branch, how its armed merge ended,
  its merge, and why a green head is not merging (with "Hands off" while a candidate holds it).
- Each event is typed once, by key, kept in `<data>/pr-notices.json`.
- A session quiet for 45 minutes with an open pull request gets a check-in; a pull request with no
  progress for 60 minutes while its CI is idle is stalled: told once and shown on the task list
  (`pr_stalled`).
- `yulius wait` has no limit while something it waits for is under way, and says why when it ends
  on its limit; with a merge armed it ends on its head's failure.
- The PreToolUse guard refuses `gh pr merge` and `gh api …/pulls/N/merge`.

## Alternatives

- **Fix `yulius wait` only**: the agent must still start it, and Claude Code's paused check-ins and
  an ended shell still leave the task deaf.
- **Read Claude Code's "check-ins paused" notice and resume them**: depends on Claude Code's
  wording and still needs something to type; a quiet-session rule needs neither.
- **GitHub webhooks**: Yulius runs the CI and the queue itself, so its records know first; webhooks
  would add an inbound endpoint per instance and still miss candidates, which exist only in Yulius.
- **Tell a person instead**: the request is that nobody has to watch; the task can act.
- **A database table for told events**: a migration for a small, per-process cache; the JSON file
  follows `armed-merges.json` and `merge-trains.json`.

## Consequences

- An idle task with an open pull request can get a turn every 45 minutes (a check-in that usually
  ends at once): a small cost against hours of nobody noticing.
- A task waiting in `yulius wait` may hear an event twice (the wait's line and Yulius's message).
- Tasks need no wait to hear about their pull request; the briefing and the delivery skill say so.
