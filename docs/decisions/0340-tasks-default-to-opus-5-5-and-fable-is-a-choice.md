# 0340. Tasks default to Opus 5.5 at high effort; Fable is a choice

## Status

Accepted (2026-11-15). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (MDL-001 to
MDL-003); task #472. Supersedes the model of
[ADR-0289](0289-tasks-default-to-fable-5-2-at-max-effort.md) and of
[ADR-0295](0295-the-default-effort-is-high-again.md) (the effort, `high`, stands). Amends
[ADR-0312](0312-a-fable-run-falls-back-to-opus-5-5-while-no-subscription-can-run-fable.md): the
default model is no longer a Fable model, so a default task has no fallback.

## Context

Fable 5.2 was the default model since 2026-11-10 (ADR-0289). Measured on the production instance
for the pull requests merged on 12 and 13 Nov 2026 (the person's figures, SPEC-493):

- Fable runs on a weekly allowance of usage credits per subscription. With Fable as the default
  for all work, the allowance was used up on 21 of 24 subscriptions within about three days.
  From then on tasks ran Opus 5.5 by the fallback (ADR-0312), and so did their reviews.
- Authors made about 57 % of Fable's output tokens and 70 % of its cached reading; reviewers
  37–38 % and about a quarter.
- A Fable 5.2 reviewer found 8 of 8 blocking defects in a blind replay of four real changes; an
  Opus 5.5 reviewer found 6 of 8 and approved two changes that needed fixes.
- Opus authors answered a request in about 6 s against 14 s, and used about half the model time
  per pull request (a median of 14–17 min against 27–33 min). Whether Opus writes worse code was
  not measured.

The person decided (2026-11-13): Opus 5.5 writes, and Fable 5.2 reviews the first two rounds of
a pull request. This record is the first half, the default. The reviewer's model is the next
slices of SPEC-493 and gets its own record.

## Decision

- `DEFAULT_CLAUDE_MODEL = "claude-opus-5-5"`; `DEFAULT_CLAUDE_EFFORT` stays `"high"`. A task that
  names no model, a Cockpit and both Superagents start with `--model claude-opus-5-5 --effort
  high`. As before (ADR-0024), a task record keeps `None` for "Default" and every launch applies
  the constant: nothing stored changes, and a running default task takes Opus 5.5 at its next
  launch.
- What is derived from the constant follows it: the first model the composer offers and its
  "Default" label, `task_create`'s descriptions, the model family a default task is placed by
  (Opus: the weekly Opus window binds it beside the 5-hour and weekly ones), what "limited" and
  "available" mean for a subscription, and the Claude Code release check (a release that does not
  know Opus 5.5 never runs in a sandbox).
- **A Fable model is a choice, and it is kept.** A task set to `claude-fable-5-2` or
  `claude-fable-5-1`, and a Superagent a person switched with `/model`, run it. While no
  subscription can run it they fall back to Opus 5.5 exactly as ADR-0312 decided.
- **A default task has no fallback.** `fallback_model` answers none for Opus 5.5: a default task
  whose model no subscription can run waits for the earliest reset, as an Opus task always did.
- **The pages watch Fable for the fallback line, not the default.** The Capacity panel
  (`fallback_model`) and the Status line say "Fable 5.2 is used up on every subscription: tasks
  set to it run on Opus 5.5 until it can run again". "Available now" counts for the default
  model.
- **`REVIEW_CLAUDE_MODEL = "claude-fable-5-2"`** names the model the independent review will
  run. It is one of the offered models, so every Claude Code release is read for it; a release
  that lacks it is reported as lacking "the independent review's model" and is not refused.

## Alternatives

- **Keep Fable as the default and add subscriptions.** The allowance is per subscription and was
  used up on 21 of 24 in three days; the fallback then ran Opus for everything, reviews included,
  which is the worst split: the weaker reviewer where the review finds defects.
- **Fable at a lower effort for authors.** Not measured, and it still spends the credits the
  review needs.
- **An instance setting for the default model.** The default changed three times in a week, so
  the case for a setting is stronger than when ADR-0295 left it out. Still not built here: the
  policy is two models with roles (who writes, who reviews which round), and one "default model"
  setting would not express it. Worth its own task once the roles have settled.
- **Let a default task fall back to another model when Opus is used up everywhere.** Nobody
  asked, and no model below Opus 5.5 is one a task should silently run on.

## Consequences

- Default tasks and the Superagents answer faster and leave Fable's credits to the review. Their
  code may be worse than Fable's was; with the reviewer held at Fable for the first round, the
  first review's verdict measures it (SPEC-493, Decisions).
- Default tasks are now bound by the weekly Opus window on each subscription. A subscription
  that is only out of Fable's credits takes default tasks.
- A person who wants Fable for a task picks it in the composer or Settings, or changes a running
  task's model; it then falls back as before.
- Running tasks and Superagents started on Fable keep it until their next launch. Production
  gets the change with the next promotion to `main` and update.
