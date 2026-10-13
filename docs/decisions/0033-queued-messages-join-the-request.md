# 0033. Messages queued into a waiting task join its request

## Status

Accepted. Specified in [SPEC-052](../../.ai/specs/SPEC-052-even-if-a-task-is-queued-i-d-like-to-be.md).

## Context

People create a task, then realise it needs correcting while it still waits: queued for a session
slot, queued after another task, or paused before its first start. A waiting task has no session,
so nothing can be typed into it. Once it runs, Claude Code queues typed messages by itself.

The task's request is its stored prompt. When the first session starts, the supervisor moves the
task to `starting` with a compare-and-set and only then reads the prompt. From that prompt it
writes the request file and the `/goal`, or the first message in prompt mode. Several other
readers use the same prompt: the scope check on typed prompts, `task_get` and the project export.

## Decision

A queued message is appended to the task's prompt, under "**Added by {login} before the task
started ({UTC time}):**". This happens only while the task has never started: its status is
`queued`, `chained` or `waiting_capacity`, with no attempt and no provider session. The append is
one conditional `UPDATE`. The start's compare-and-set to `starting` either comes after it, and
reads the message, or makes it match no row, and the person gets `409`. The whole request keeps
the creation limit of 64 KiB. The task view shows the request and a box to queue a message.

## Alternatives

- **A table of queued messages, merged into the request at start.** It keeps messages apart for
  display, but every reader of the request would need the merge. One left out (the scope check,
  `task_get`) would act on the uncorrected task.
- **Editing the request in place.** It is flexible, but it loses what was first declared and
  needs conflict handling between two editors. The person asked to queue messages.
- **Accepting messages after the start too, typed into the session.** Claude Code already queues
  typed input. A resumed session never re-reads the request, and typing into a session unasked is
  what [0023](0023-no-messages-until-delivery-is-reliable.md) withdrew.

## Consequences

Every reader of the request sees the correction with no extra code. The request is append-only
while the task waits, and each addition names who added it and when. A message cannot be taken
back: a person who changes their mind queues another one, and later lines correct earlier ones.
A task that started takes no queued message; its session is the way in.
