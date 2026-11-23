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

Record an observed open PR before preparing advice about its merge queue state. Use the slot’s request generation, count of successful takes and decision state to check that the same queue request is still current. A pass overtaken by a queue decision can then defer typing and still recognize the PR after its merge.

During restoration, keep the open PR sighting and withhold a check-in until its owner's saved merge request has a live activation. The train retains the member's admission head and checkpoints the head verified against its predicted tree when GitHub needs more time. Queue-position reads can then distinguish a verified move from an incomplete checkpoint. The conflict files of an excluded member belong to the activation and head that produced them.

A failed candidate narrowed to one PR belongs to that PR’s merge decision. The source run may have tested the same tree for a larger candidate. The queue persists the accepted decision before clearing its original arm. Restoration reconciles the recorded activation with the arm saved on disk. The watcher reuses the live outcome key after restart so a reported decision stays heard. A new ordinary rerun holds replay for up to the queue’s 15-minute wait from its first watcher observation. The notice record binds that time to the queue decision and ordinary run. Changing queue priority or restarting retains the deadline. A singleton that reuses a larger train’s CI run has its own verification notice. Verify the hosted head and integration base together and include the failing job’s retained, redacted diagnostic in the owner’s merge outcome. [SPEC-558](../../.ai/specs/SPEC-558-parked-codex-tasks-no-codex-exit-screen.md)

The same queue decision holds the live owner’s session slot. The watcher verifies hosted inputs outside the launch lock. Capacity accounting uses a recent observation for the same task branch and saved decision. A launch holds the owner’s delivery guard from its final check through reservation. Before changing an observation the watcher takes the exclusive guard briefly, releasing it before contacting GitHub. A busy watcher retries on a later pass. A failed read or restart leaves the owner occupying its slot.

- A server-side watch (every 20 s) looks at each running task with an open pull request, from
  Yulius's own records and clone, and types what needs the task into its session as one message
  with the next step: a red run at its head or of a merge candidate holding it (jobs, failing
  tests, where to read the log), a conflict with the integration branch, how its armed merge ended,
  its merge, and why a green head is not merging (with "Hands off" while a candidate holds it).
Use the queue’s owner-preferred ordinary CI result at the armed head. A shared ordinary failure can reach its owner even when the train registry or candidate run cannot be read. Verify a candidate against the active request and its current members because the queue can reuse a CI run for the same tree. Use a bounded hosted PR read for an armed notice that is due. The author’s remote-tracking ref can lag a push from another clone. Recheck a held scope decision before dispatch and leave deferred notices unheard.

- The first paste of a live PR notice takes the writer only if it is free. A waiting paste could answer a decision that appeared during a backpressured resize. The existing watch retries the unheard event and checks the owner again.
- Each event is typed once, by key, kept in `<data>/pr-notices.json`.
- A session quiet for 45 minutes with an open pull request gets a check-in; a pull request with no
  progress for 60 minutes while its CI is idle is stalled: told once and shown on the task list
  (`pr_stalled`).
- `yulius wait` has no limit while something it waits for is under way, and says why when it ends
  on its limit; with a merge armed it ends on its head's failure.
- The PreToolUse guard refuses `gh pr merge` and `gh api …/pulls/N/merge`.

The parking supervisor uses the latest singleton decision for the original owner. A changed decision during verification leaves the owner live for the next pass. See SPEC-558.

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
