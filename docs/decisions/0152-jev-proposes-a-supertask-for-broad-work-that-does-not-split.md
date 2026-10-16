# 0152. Jev proposes a supertask for broad work that does not split

## Status

Accepted. Specified in [SPEC-188](../../.ai/specs/SPEC-188-under-the-supertask-add-one-shotting-the.md)
(task #173). It extends task intake (SPEC-002 ADR-16) and supertasks
([0054](0054-a-supertask-holds-its-project-at-the-start-door.md)).

## Context

A supertask waits for its project's running tasks, then runs alone. It is meant for broad work
such as security hardening or one-shotting the product. People start such work with Start now as
well, and it then collides with every task running beside it. The person asked that Jev "stop the
user and propose this type when different is used but the work is really really broad", and that
a request like "fix all GitHub issues" is not in this category, because intake splits it.

Task intake already asks Jev one fixed Choice question per request, with fixed thresholds, and
the default wins when unsure (ADR-16). The composer calls `/intake/check` on Start, and the split
card stops the start for a request that mixes workstreams.

## Decision

- **A fixed `breadth` question.** `/intake/check` asks Jev `focused` or `broad` about every new
  task's request of 3 words or more, at the same time as the split question (40 words or more),
  each within the check's 2.5 s deadline. The question names the examples of broad work (security
  hardening, one-shotting the product, a project-wide refactor, migration or upgrade). It also says
  that a list of separate items, bugs or issues ("fix all open GitHub issues") is focused, because
  it is split into separate tasks.
- **A split wins.** The answer carries `"supertask": true` only when Jev is confident (0.85, fixed,
  no setting) that the request is `broad` **and** the split verdict is not `several`.
- **The composer stops the start.** Started with Start now, Run after or a rule, a request with
  `supertask` is not created. The card "Run it as a supertask?" takes the split card's place below
  the box. It offers **Run as a supertask** (which replaces a chosen task or rule, and says so) and
  **Start as chosen**; its close button withdraws and keeps the prompt. A task started as a
  supertask or prepared is never stopped.

## Alternatives

- **A third split option `broad`.** One question would mix two independent properties in one
  distribution and change the tuned split behaviour.
- **Keywords** ("harden", "whole product"). They are brittle in both directions, and Jev is
  already the project's only classifier.
- **Enforcing on the server** (refusing or converting a broad `POST /tasks`). Agents and
  automations create tasks through the API too. The person decides in the composer, as with the
  split card; the API stays predictable.
- **Proposing for prepared tasks.** A supertask cannot be prepared, so the proposal would discard
  the person's "When I decide" or time.

## Consequences

- Every Start without Supertask makes one check call (with a Jev key, up to two Jev questions).
  Previously only prompts of 40 words or more did. Without a key the server answers at once.
  Every failure starts the task as chosen.
- `IntakeVerdict` gains `supertask`, and `checked` now means "Jev answered either question".
- The supertask proposal only nudges: the person can always start the task as chosen.
