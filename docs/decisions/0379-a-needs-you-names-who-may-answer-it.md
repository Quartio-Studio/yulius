# 0379. A needs_you names who may answer it

## Status

Accepted (2026-11-19). Specified in
[SPEC-512](../../.ai/specs/SPEC-512-marcin-s-five-issues-1412-1418-1413-1107.md) part 2; task #494,
issue #1418. Builds on [ADR-0370](0370-a-teammate-answers-a-tasks-needs-you-without-taking-control.md)
(a teammate answers without taking control) and keeps
[ADR-0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md) and
[ADR-0365](0365-a-teammate-takes-control-unless-the-owner-chose-to-be-asked.md) as they are.

## Context

- A task can need decisions from two people. On a company instance a review asked its owner for
  some decisions and an engineer for others in one request; the engineer could only answer by
  taking the task, which moves its GitHub identity, or by asking the owner to pass the answer on.
- ADR-0370 lets anyone with write access answer a running task's request on its page. The request
  cannot say whom it asks, the engineer's own agent cannot answer through `task_message` (the
  owner's), an answer meant for one question can land on the next, and the owner cannot take back
  a permission they never gave.
- Taking control (ADR-0365) is for steering a task, not for one answer.

## Decision

- **`needs_you` names `who`.** One to ten logins besides the owner, each a person of the instance
  with write access to the project, checked when the request is raised; one that is not refuses
  the whole request with the reason. Stored with the request and shown with it.
- **A named request narrows who answers.** Its owner and the people still named answer it; others
  are refused with whom it asks. A request that names nobody keeps ADR-0370's rule.
- **`task_message` carries an answer.** From someone who is not the owner, while a request is open
  that they may answer, a message is that answer: the same label, guards (session, draft, the
  owner's personal reach, a terminal question) and audit as the page's. Otherwise it is refused as
  before.
- **An answer names its request.** It carries the request's `at`; after the request ended, after a
  new one replaced it, or after the owner withdrew the person, it is refused with which of these
  happened.
- **The owner withdraws.** On the task's page, or with an operate token, the owner removes a name;
  audited `task.needs_you.withdrawn`. The last name gone, the request is the owner's alone.
- **Nothing else moves.** Terminal input, Continue and merges stay the owner's; stop, archive and
  delete stay anyone's who may work in the project (ADR-0331).

## Alternatives

- **A temporary hand-over per question.** Moves the GitHub identity and personal reach for one
  answer, which the issue rules out.
- **Naming only for notification, anyone answers.** Leaves an answer meant for one person open to
  all, and gives the owner nothing to withdraw.
- **`task_message` from any writer.** Lets a message that is not an answer steer someone else's
  task; ADR-0331 keeps steering the owner's.
- **Refusing names without write access silently.** The agent would believe it asked someone who
  can never answer.

## Consequences

- An agent that needs two people says so, and each answers from their own page or agent; the
  transcript and audit say who answered.
- An unnamed request is unchanged; a named one is narrower than ADR-0370's.
- The owner sees and controls who besides them answers.
