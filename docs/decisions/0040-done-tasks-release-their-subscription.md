# 0040. Done tasks release their subscription

## Status

Accepted. Specified in [SPEC-057](../../.ai/specs/SPEC-057-tasks-marked-as-done-are-still-attached.md)
(task #48). It extends SPEC-050 SES-101 to the subscription pool and keeps TSK-014.

## Context

A Done task keeps its Claude Code session attached and idle, so a follow-up prompt continues it
(TSK-014). Its attempt therefore stays `live`. Every subscription count was taken over the live
attempts: the Subscriptions page ("N tasks on this subscription"), the Status panel, the capacity
view, the pool's per-subscription load and the guard on removing a subscription. A subscription
whose tasks were all Done looked busy and could not be removed until each task was stopped or
archived by hand. SES-101 had already stopped counting Done tasks against the session limit.

## Decision

- A subscription is held by its **working** tasks: live attempts except those of Done tasks, by
  the session limit's rule (`task_is_done_locally`). One helper, `App::working_attempts`, feeds
  the session limit, `in_use_by` everywhere and the pool's load.
- Removing a subscription is refused while a working task uses it. Done tasks' idle sessions on
  it are closed first (`closed`, as archiving a Done task does), then it is deleted.

## Alternatives

- **Close every Done session automatically.** It frees the subscription without a rule, but it
  breaks TSK-014: the idle session is there for follow-ups.
- **Only hide Done tasks from the card.** The card would say "No tasks" while Remove still failed
  and the pool still treated idle sessions as load.
- **Delete the subscription and leave Done sessions running.** A session would keep a credential
  Yulius no longer has and report usage for a subscription that no longer exists.

## Consequences

The cards, the Status panel, the capacity view and the session limit agree on what is busy. A
Done task typed into again counts again. "Done" is the local rule of SES-101 (goal achieved, or
plan complete with the change merged): a task Done only because its open pull request is green
still holds its subscription, as it holds a session slot. Removing a subscription with Done tasks
on it closes them together, so it takes up to one close escalation (about 25 s), and they end
`closed`; **Continue** starts them again
on another subscription. Counts are read when the page loads; a task becoming Done does not push
an update to an open Subscriptions page, as task starts and ends never did.
