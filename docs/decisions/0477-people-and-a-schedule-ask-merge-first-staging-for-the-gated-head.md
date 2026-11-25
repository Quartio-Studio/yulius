# 0477. People and a schedule ask merge-first staging for the gated head

## Status

Accepted for [SPEC-650](../../.ai/specs/SPEC-650-merge-first-staging-a-person-and-their-s.md)
(task #634). Amends [0411](0411-staging-changes-through-a-queue-of-tested-merged-revisions.md):
the queue no longer takes requests from tasks only, and a person's plain deploy of the
merge-first target becomes a request instead of a refusal.

## Context

Under ADR-0411 only a task could change the merge-first staging target, and only once its whole
plan was done and merged. Each task could do this once. People could not ask at all: the CD
page's "Deploy dev" was disabled and their Superagent's `cd_deploy` was refused. When no task
finished its plan, merged green revisions sat for hours. Production waited too, since it
promotes only the revision staging verified (SPEC-575 MFS-009). Task notes and needs-you cards
still told people to press the disabled button.

## Decision

- **A person's request is a task-less staging request** for the verified integration head
  (`requested_by` = `user:<id>`), recorded from the CD page, the page's plain deploy of that
  target, or the Superagent's `cd_deploy` once the person approves it on the panel with the
  typed slug. Write access is enough. It goes through the same queue: the full gate on exactly
  that commit, revision order, one deploy at a time, restore after a failed deploy.
- **A red head waits for the next green one.** A task-less request whose gate failed becomes
  `inherited`. When the head moves on, Yulius records one task-less request for the new head.
  When a revision containing the waiting request is verified, that request is `included`.
- **A schedule keeps staging moving.** A merge-first `[cd.<name>]` takes `serve_every_minutes`
  (default 60) and `serve_after_merges` (default 5; `0` turns either off). When staging is behind
  the head and nothing is pending, Yulius records a scheduled request (`yulius:schedule`). That
  deploy acts as the project's creator, or else as the newest landing's owner, whose write
  access is checked.
- **A task's own request keeps its rules** (whole delivery, one successful deploy) for its own
  bookkeeping only.
- **The server decides each deploy control** (`deploy_action`) for the viewer's access. The CD
  page renders it, and `needs_you` refuses a card that sends the task's owner to a control
  disabled for them.

## Alternatives

- **Let a person deploy the head directly.** Rejected: an untested revision could reach staging,
  and production's pin to the verified revision would break.
- **Deploy the newest older green revision at once when the head is red.** Rejected: the person
  asked for what is merged now. The schedule still moves staging forward.
- **A new request state for "waiting for the next green revision".** Rejected: `inherited`
  already waits without blocking newer requests and is included by a later verified deploy. A
  new state needs a table rebuild.
- **Only rewording the guidance.** Rejected: the stall itself (no task finishing its plan) would
  remain.

## Consequences

- Staging follows the integration branch without a task, at the cost of at most one full gate
  run per scheduled request on a revision that has none.
- The CD page and Superagent can always ask for staging. What they wait for is shown in plain
  words.
- A project that wants the old behaviour sets both `serve_*` keys to `0`. People can still ask.
