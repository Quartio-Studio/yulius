# 0312. A Fable run falls back to Opus 5.5 while no subscription can run Fable

## Status

Accepted. Specified in [SPEC-482](../../.ai/specs/SPEC-482-add-fallback-for-opus-5-5-if-fable-is-do.md);
task #464. Amends [ADR-0301](0301-a-models-usage-credits-are-a-limit-of-that-model.md) (its
rejected alternative "switch the task to another model", and what leaving a subscription out
means), SPEC-001 ROT-010 and [ADR-0289](0289-tasks-default-to-fable-5-2-at-max-effort.md) ("a
subscription without Fable access cannot run a default task").

## Context

Fable 5.2 is the default model. It runs on a weekly allowance of usage credits per subscription;
used up, Claude Code refuses the model and Yulius keeps that as the model's own weekly window on
the subscription (ADR-0301). A task then moves to a subscription that can still run Fable. When
none can, it waited for the earliest reset, up to a week, or asked a person when nothing said
the end of the limit, while every subscription still had room in its 5-hour and weekly windows
and could run Opus 5.5. The person asked (2026-11-12): "add fallback for opus 5.5 if fable is
done everywhere".

ADR-0301 rejected switching the model because "the person chose the model, and another
subscription can run it". That reason does not cover the case that no subscription can.

A task's setting was all that said what a run runs: a limit was attributed to the task's model
family. With a run launched on another model than the task's, that is wrong.

## Decision

- **The pool policy decides the model at each placement.** `rotation::placing_family`: the
  task's own model family while an account is a candidate for it, else its fallback's while an
  account is a candidate for that, else its own (the task waits). `RotationInput` carries the
  fallback's family. Every trigger that places a run uses it; a wait ends at the earliest time
  either model can run.
- **The fallback is Opus 5.5 for every Fable model, chosen or default**
  (`harness::fallback_model`, `FALLBACK_CLAUDE_MODEL = "claude-opus-5-5"`): one constant by its
  full id, one of the offered models, no setting. No other model falls back, and there is no
  chain.
- **The task keeps its model; the launch gets the fallback.** `tasks.model` is not changed.
  `choose_account` returns the account together with the fallback model from the same snapshot
  (`Placed`), and the launch passes it to Claude Code (`--model`, with that model's effort).
- **A run records the model it was launched with** (`attempts.model`, schema version 54; a
  Superagent's `shell.json`). A limit reported to a run binds that model's family. A run stays
  on its subscription while that subscription can run the model the run was launched with.
- **The current subscription is a candidate when the model changes.** A placement after a limit
  leaves the current account out only when the run placed would be of the model the current run
  was launched with. One subscription is enough: the session continues on it with Opus 5.5.
- **No restart to get back.** A run on the fallback is not restarted because Fable can run
  again. The next launch that happens anyway (a move at 98 %, a limit, a pause that resumes,
  Continue, a person's move, a restart for a newer Claude Code) is placed with the task's own
  model when a subscription can run it.
- **A subscription is left out as out of the refused model** (SPEC-464 UCL-010,
  `decide_leaving_out`): its model's own window counts as full with no reset, not the whole
  subscription as limited. The fallback's known reset on it is then a reset to wait for. A pool
  that is in fallback anyway leaves nothing out, and a placement with the fallback that only
  leaving out would make is not taken, because the launch asks the policy without it.
- **It is said, and nobody is asked.** The session's terminal says the change in one line at
  the launch that makes it, both ways; the move's entry names the model; the task's details,
  the Capacity panel (`fallback_model`) and the Status line say that sessions run on the
  fallback. A fallback that is refused with no end too is SPEC-464's case for both models: one
  retry, then a person.
- **The Superagent follows the same rule** (`place_default_run`).

## Alternatives

- **Store the switch on the task** (set `tasks.model` to Opus). The person's choice would be
  lost and a way back would need its own state and trigger. Rejected for a decision per launch.
- **Fall back per subscription** (run Opus on any subscription that is out of Fable). A task
  would run Opus while another subscription can run Fable; the request says "everywhere".
  Rejected.
- **Only tasks of the default model.** A task set to Fable 5.2 by name would wait a week beside
  default tasks that go on. The request names the model. Rejected.
- **Go back to Fable at the next turn boundary.** A restart ends background shells and pays for
  a cold prompt cache, and a limit nothing says the end of is ended by each provider reading: the
  run would be refused and fall back again at every reading. Rejected for the next launch.
- **Keep the launch model in memory, or read it from the transcript's last answer.** A restart
  of Yulius would attribute a limit of Opus to Fable and send the task back to the same
  refusal; the transcript names a model only after the first answer. Rejected for the column.
- **A setting for the fallback model or to switch it off.** Nobody asked; the default effort's
  setting was left to its own task the same way (ADR-0295). Left out.
- **Keep leaving a subscription out as limited as a whole.** With one subscription whose Opus
  window says when it resets, the wait had no known end (the mark hides that reset), so the task
  was started on it again at every reading. Rejected for the model's window.

## Consequences

- Tasks and Superagents go on with Opus 5.5 when Fable's allowance is used up on every
  subscription: weaker answers than Fable's, at about half the price per token, instead of a
  wait of up to a week. Their sessions say so.
- A session that fell back runs Opus 5.5 until its next launch, also after Fable can run again.
- A subscription that never runs Fable (no access, credits switched off) and says no reset
  still gets one refused request from each Fable task placed on it after a provider reading;
  the task then goes on with Opus 5.5 on it instead of waiting.
- A task waits or asks a person only when neither model can run: for a model with no fallback
  as before, for a Fable model after both were refused.
- A resumed session continues on another model. Claude Code offers that itself at this refusal
  ("/model to switch models"); Yulius relies on it.
- Schema version 54 (`attempts.model`): an older Yulius cannot open the database after the
  update (as with every migration; the update's own rollback applies, ADR-0264). Runs recorded
  before it have no model and count as their task's.
- "Run #N on…" lets a person choose a subscription that is out of Fable while no subscription
  can run it; the session then runs Opus 5.5 there.
