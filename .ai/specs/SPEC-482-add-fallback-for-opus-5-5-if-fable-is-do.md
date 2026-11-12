---
yulius-task: 77d1f091-c74f-43f0-83c5-d4f0ef9e27a6
status: in-progress
---

# SPEC-482: a Fable task runs Opus 5.5 while no subscription can run Fable

Status: in-progress
Date: 2026-11-12
Owner: Yulius task #464
Scope authorization: the request of task #464 (by MrFishPL, 2026-11-12): "add fallback for opus
5.5 if fable is done everywhere". Amends [ADR-0301](../../docs/decisions/0301-a-models-usage-credits-are-a-limit-of-that-model.md)
(its rejected alternative "switch the task to another model") and SPEC-001 ROT-010; decision in
[ADR-0312](../../docs/decisions/0312-a-fable-run-falls-back-to-opus-5-5-while-no-subscription-can-run-fable.md).

## Goal

When no connected subscription can run a task's Fable model (its weekly allowance of usage
credits is used up on every one of them) and one can still run Opus 5.5, the task goes on with
Opus 5.5 instead of waiting for a reset or asking a person: at its start, at the limit that cut
its turn, and when it resumes. The same holds for the Superagent. People see which model a
session runs and why. Fable is used again by itself once a subscription can run it.

## Context

- **What happens today.** Fable 5.2 is the default model (ADR-0289). It runs on a weekly
  allowance of usage credits per subscription; used up, Claude Code refuses the model ("You're
  out of usage credits. /model to switch models."). Yulius reads that as a usage limit of the
  model (SPEC-464, ADR-0301): the subscription's `weekly_fable` window is full until its reset
  and the task moves to a subscription that can still run Fable. When none can, the task waits
  (`limit` pause) for the earliest reset, up to a week, or asks a person when nothing says the
  end of the limit (`task.limit_without_end`), although every subscription still has room in its
  5-hour and weekly windows and could run Opus 5.5.
- **What ADR-0301 decided.** "Switch the task to another model" was rejected there because
  another subscription could run the model. This request is the case that reason does not
  cover: no subscription can.
- **What exists to build on.** The pool policy is a pure function
  (`yulius_domain::rotation::decide`) of the accounts' usage and the task's model family
  (`ModelFamily`, whose own weekly window binds beside the 5-hour and weekly ones). Every
  decision for a task builds its input in `App::rotation_input`; a launch asks the policy again
  for its subscription (`choose_account`) and gives Claude Code `--model` from the task
  (`HarnessLaunch::model`). A session resumes natively (`claude --resume`) on a move; the model
  is a flag of each launch, and Claude Code continues a session on another model (it offers
  `/model` at this very refusal). The Superagent ("Cockpit") takes its subscription from the
  same policy (`pool_account`) and runs the default model.
- **What is missing.** A run does not record the model it was launched with: the task's setting
  is all there is, so a limit is attributed to the task's model family (`task_family`). Once a
  run can be launched with another model than the task's, that is wrong: a limit of Opus hit on
  the fallback would fill `weekly_fable`.
- **Wording.** "Fable is done everywhere" is read as "on every subscription". Every place that
  runs Fable by the pool's choice is covered: tasks and the Superagents. A split proposal runs a
  small model that no model's window binds, and is not touched.

## Requirements

- FBK-001 (the fallback model). `yulius_domain::harness::fallback_model(model)` names the model a
  run falls back to: `claude-opus-5-5` (`FALLBACK_CLAUDE_MODEL`) for a model of the Fable family,
  none for any other model. The fallback is one of `OFFERED_CLAUDE_MODELS`, so the check that
  Claude Code knows the offered models (SPEC-461) covers it.
- FBK-002 (placement). The pool policy places a run **with the fallback** when no account is a
  candidate for the task's model family and at least one is a candidate for the fallback's
  (`rotation::placing_family`): the candidates are then those of the fallback's family, ranked as
  always. It holds for every trigger that places a run (`Start`, `LimitReached`, `Proactive` once
  the run cannot stay, `Manual`). With a candidate for the task's own family, nothing changes.
  With a candidate for neither, the task waits as before, until the earliest known time an
  account can run either model.
- FBK-003 (the same subscription). A run that changes its model may stay on its subscription:
  the current account is left out of a placement only when the run placed would have the family
  the current run was launched with (a limit of that model on that account). So one subscription
  is enough: the session is continued on it with Opus 5.5.
- FBK-004 (no restart for the model's sake). At a turn boundary a run stays while its
  subscription is a candidate for the model **the run was launched with**. A run on the fallback
  is not restarted because Fable can run again; a run on Fable whose subscription can no longer
  run Fable is placed anew (FBK-002), as before. The next launch that happens anyway (a move at
  98 %, a limit, a pause that resumes, Continue, a person's move, a restart for a newer Claude
  Code) is placed with the task's own model again when an account is a candidate for it.
- FBK-005 (launch and record). A launch placed with the fallback gives Claude Code the fallback
  model and the effort that model takes (`claude_effort`); the task's own setting (`tasks.model`)
  is not changed. Every run records the model it was launched with (`attempts.model`, migration
  0054; `NULL` for runs before it, read as the task's model).
- FBK-006 (a limit binds the run's model). A usage limit reported to a run is recorded for the
  family of the model that run was launched with, not the task's: a limit of Opus hit on the
  fallback fills `weekly_opus` and leaves `weekly_fable` as it is.
- FBK-007 (left-out subscriptions). A subscription the pool keeps a task off because it refused
  the task's model at a limit nothing says the end of (SPEC-464 UCL-010) is kept off only
  placements with the task's own model family; a placement with the fallback may use it.
- FBK-008 (said in the terminal and the history). The session's terminal says in one line that
  it continues on the fallback and why ("Fable 5.2 is used up on every subscription: this
  session continues on Opus 5.5."), and in one line when a later launch runs the task's model
  again ("Fable 5.2 can run again: this session continues on it."). The move's entry under the
  task's switches names the model the session moved to when it is the fallback. Nobody is asked
  anything: no Needs you, no `task.goal_retries_spent`, no `task.limit_without_end`.
- FBK-009 (the task view). `TaskDetail` gains `fallback_model`: the model the task's live run
  was launched with when it is not the task's own, else `null`. The task view shows it beside
  the model ("Opus 5.5 while Fable 5.2 is used up").
- FBK-010 (the capacity view). `GET /api/capacity` counts as `available` the subscriptions a
  task of the default model can be placed on now, with its own model or the fallback; an
  account's `available` says the same; `resumes_at` is the earliest time either model can run.
  It gains `fallback_model`: the fallback while such a task would be placed with it, else
  `null`. The Capacity panel then says "Fable 5.2 is used up on every subscription: tasks run
  on Opus 5.5 until it can run again", and the Status line for "every subscription is at a
  usage limit" says the same instead of "tasks continue when a window resets".
- FBK-011 (the Superagent). A Superagent or Global Superagent is placed by the same rule
  (`pool_account` for the default model): started, restarted or moved at a limit while no
  subscription can run Fable, it runs Opus 5.5, on the same subscription when that is the one
  with room (FBK-003). Its limit is recorded for the family of the model it was launched with
  (FBK-006), and its terminal says the line of FBK-008.

Not in scope: a setting to switch the fallback off or to choose its model; a fallback for other
models, or a chain of fallbacks (Opus 5.5 to Sonnet); following a model a person switched inside
the session with `/model` (as before, ADR-0301); moving a run back to Fable before its next
launch; the price shown for a run, which already follows the model that answered.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] The pool policy places a Fable run with Opus 5.5's family when no subscription can run Fable (FBK-001 to FBK-004, FBK-007)
- [ ] A run records the model it was launched with: migration 0054 and the attempt record (FBK-005)
- [ ] A task's launch takes the placed model, says so, and a limit binds the run's model (FBK-005, FBK-006, FBK-008)
- [ ] The task view and the capacity view say that sessions run on the fallback (FBK-009, FBK-010)
- [ ] The Superagent falls back by the same rule (FBK-011)
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- AC-1 (FBK-002, FBK-003, FBK-005, FBK-008). One subscription, a default task; Claude Code
  refuses Fable for its usage credits. The same session continues on that subscription, launched
  with `claude-opus-5-5`, past the refused turn; the terminal says the fallback line; nothing is
  under Needs you and no goal pause is audited.
- AC-2 (FBK-002). Two subscriptions, Fable used up on the first only: the task moves to the
  second and runs Fable, as before this change.
- AC-3 (FBK-002, FBK-005). With Fable used up on every subscription, a new default task starts on
  Opus 5.5 at once; a task set to Opus or Sonnet is placed as before; a task set to
  `claude-fable-5-1` falls back too.
- AC-4 (FBK-004). A run on the fallback stays at its turn boundary once Fable can run again; its
  next launch (Continue after a stop) is launched with the task's model and says so.
- AC-5 (FBK-006). A limit of Opus reported to a run on the fallback fills `weekly_opus`;
  `weekly_fable` keeps its reset.
- AC-6 (FBK-002). With every subscription at its 5-hour limit and Fable used up, the task waits
  until the earliest 5-hour reset, not the allowance's.
- AC-7 (FBK-009, FBK-010). The task's detail names the fallback model while its run is on it; the
  capacity view counts the subscriptions that can take a default task on the fallback and names
  the fallback; both panels say it in words.
- AC-8 (FBK-011). A Superagent whose turn is cut by the Fable limit on the only subscription
  starts again on it with Opus 5.5 and continues the turn.
- AC-9 (FBK-005). A database of schema version 53 opens with `attempts.model` `NULL` for its
  runs, and such a run's limit is attributed to the task's model.

## Test plan

- Unit, `yulius-domain` (`rotation`, `harness`): the placing family in each pool state (own
  candidates, only fallback candidates, neither, no fallback for the model); every trigger in
  fallback; the current account kept or left out by the run's family; stay by the run's family;
  the wait's `until` over both families; `decide_leaving_out` in fallback; the fallback model
  per model id and that it is offered.
- Store (`crates/yulius-store/tests/store.rs`): `attempts.model` round trip; migration from
  schema version 53.
- Integration with real processes and the fake agent (`crates/yulius/tests/model_fallback.rs`,
  beside `usage_credits.rs`): AC-1 to AC-5 for tasks by what each launch asked for
  (`fake-launch.txt`), the stored usage, the notices and Needs you; the API views of AC-7;
  the Superagent of AC-8 in `cockpit.rs`.
- Web (`e2e/tests`): the task view's model line and the Capacity panel's line against the mock.
- Not testable here: a real Claude Code resuming a Fable session with `--model claude-opus-5-5`
  against Anthropic's API (needs a subscription whose Fable allowance is used up). The flag and
  the resume are what Yulius uses for every move today; what the test proves is the launch
  Yulius asks for.

## Decisions

- **The fallback is decided by the pool policy at each placement, not stored on the task.** The
  task keeps the model its person chose; each launch is placed with what can run now. A stored
  switch would need its own way back. (ADR-0312)
- **Only when no subscription can run the task's model.** While one can, the task moves there
  (ADR-0301 stands): the person's model is kept as long as anything can run it.
- **Every task of a Fable model, chosen or default.** The request names the model, not the
  default; a week's wait for a model nobody can run is what the request ends.
- **Opus 5.5 by its full id, one constant, no setting.** As the default model (ADR-0024): an
  alias would move with each release. A setting is left to its own task, as for the default
  effort (ADR-0295).
- **A run records its launch model; a limit binds that.** Without it a limit of Opus on the
  fallback would be stored as Fable's and the task would be sent back to the same refusal.
- **No restart to get back to Fable.** A restart ends background shells and pays for a cold
  prompt cache, and a limit nothing says the end of is ended by each provider reading: a run
  that went back at once would be refused and fall back again at every reading. The next launch
  that happens anyway uses Fable when it can run.
- **The same subscription is a candidate when the model changes.** Leaving the current account
  out protects from a limit that binds the placed run; it does not bind a run of another model.
