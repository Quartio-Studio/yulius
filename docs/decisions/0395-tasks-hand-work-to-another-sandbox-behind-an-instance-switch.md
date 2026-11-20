# 0395. Tasks hand work to another sandbox behind an instance switch

## Status

Accepted (2026-11-20). Specified in
[SPEC-512](../../.ai/specs/SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md) part 5; task #494,
issue #1106. Amends
[ADR-0227](0227-the-superagent-starts-tasks-in-other-sandboxes-only-on-its-persons-approval.md)
for task agents: a task's agent may start work in another sandbox when an instance administrator
allows it. The Superagent's tools and their approval stay as ADR-0227 and ADR-0333 say.

## Context

- A task that finds part of its work belongs in another repository can only ask a person with
  `needs_you`; the person opens the other sandbox, writes the task and carries the result back.
- ADR-0227 kept cross-sandbox tasks to the Superagent, on its person's click, because a task agent
  reads files, issues and pages that may carry injected text. Its call waits for the click, which a
  task run cannot afford.
- Teams that split a product across repositories want the hand-over, with bounds.

## Decision

- **An instance switch, off by default.** `off`, `ask` or `automatic`, with a chain's depth (1 to
  5, default 3) and handoffs per owner a day (1 to 100, default 20), set by instance
  administrators. Off, task agents are not served the tool at all.
- **A sandbox opts out.** Its writers close it to handoffs; it stays closed whatever the switch.
- **`sandbox_handoff` for task agents.** `{sandbox, title, prompt}` with `task_create`'s options.
  The checks of `sandbox_task_create` (a sandbox the owner can work in, else not found; visible
  text only; the tools' hourly bound; nothing from an automation's task, which holds nobody) plus
  depth, cycles (a sandbox already in the chain) and the daily bound. Every refusal is audited
  and says why.
- **Ask stores; automatic creates.** In ask mode the request is stored and the call returns; the
  task's owner approves or declines it on the task's page, in a browser session only, and
  approval creates exactly what was shown. In automatic mode the task is created at once.
- **The owner's task, with its source.** It belongs to the source task's owner (GIT-004), origin
  `handoff`, and shows "Handed off from <sandbox> #N". When it ends, Done or merged, the source
  task is told its outcome and pull request. `task.handoff` is audited on both sandboxes, never
  with the prompt.
- **Delivery is unchanged.** Its work reaches `dev` only through its own pull request and CI.

## Alternatives

- **Only `ask`.** Safe, but the person is in every loop; it saves typing, not attention.
- **Serve `sandbox_task_create` to tasks.** Its waiting call does not fit a run, and nothing could
  turn it off.
- **Keep provenance in the prompt.** Unverifiable and lost on edits; a stored handoff is the
  record the chain's bounds and the outcome rely on.

## Consequences

- With the switch on, a task starts the API change its website change needs and is told when it
  is done; an injected instruction meets the opt-out, the bounds, `ask` and the audit.
- A schema change (migration 0076, schema version 74): `instance.handoffs` and `task_handoffs`.
