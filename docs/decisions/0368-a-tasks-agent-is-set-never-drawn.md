# 0368. A task's agent is set, never drawn

## Status

Accepted on 2026-11-19 (`F 2026-11-19 Harness`: the person's request in task #496, "make sure
harnesses doesnt change themselves between tasks (like claude -> gemini) because it's
unpredictable. A harness is set for a task"). Specified in
[SPEC-514](../../.ai/specs/SPEC-514-make-sure-harnesses-doesnt-change-themse.md). Supersedes the
"Draw" part of [ADR-0361](0361-antigravity-is-a-third-harness-picked-at-random-by-default.md);
the rest of ADR-0361 stands.

## Context

- Since SPEC-505 slice E, a task created without an agent (the composer's default, Random), a
  follow-up an agent created with `task_create`, and every run of an automation without an agent
  drew Claude Code or Antigravity among the agents with a usable subscription. Consecutive tasks
  of the same person or automation ran different agents without anyone choosing it.
- A task already keeps its harness for life; only the choice at creation was random.
- The same person asked for the random default two days earlier, to compare the agents; they now
  find it unpredictable and want each task's agent set.

## Decision

- **No draw.** A task created without an agent (or with `auto`) runs the agent its model implies
  (Antigravity for `gemini-*`, else Claude Code), and without a model Claude Code, whatever
  subscriptions are usable; a Claude Code task without a usable Claude account is refused as
  before (`Harness::unnamed`).
- **An agent's follow-up keeps its agent.** `task_create` without an agent or a model, and
  `automation_create` without an agent, use the calling task's agent; without a calling task
  (the Superagent), Claude Code.
- **An automation names its agent.** `auto` saves as `claude`; the automations stored as `auto`
  became `claude` (migration 0068), so every run of an automation runs the same agent.
- **The UI.** The composer's and Settings' no-choice item is Default (Claude Code, Opus 5.5); the
  automation editor offers Claude Code, Codex and Antigravity, Claude Code first. A chosen model
  stays the browser's default for the next tasks, as before.
- **History.** Tasks drawn before keep `harness_choice: random` (`task_harness_draws` stays); no
  new task is drawn.

## Alternatives

- **Keep Random as an opt-in choice**: a second meaning of "no agent" (`auto` against `random`)
  for little use; the person can still compare agents by choosing one per task.
- **Fall back to the first agent with a usable subscription**: deterministic only while
  subscriptions stay as they are; a Claude subscription signing out would move the next tasks to
  Gemini, the change the person asked to stop.
- **Remember the last agent per sandbox on the server**: a new piece of state for what the
  browser's remembered model already does for a person.
- **Drop `task_harness_draws` and `harness_choice`**: would relabel the tasks that were drawn as
  chosen in the analytics.

## Consequences

- Every new task's agent is known before it starts: from the request, its model or its caller.
- An instance with only Antigravity subscriptions has to pick Gemini 4 Carbon (remembered in the
  browser) or name `antigravity`; a task on Default is refused for want of a Claude account.
- The analytics' `random` rows end with this change; comparisons need tasks chosen per agent.
