# 0301. A model refused for its usage credits is a usage limit of that model, kept until its reset

## Status

Accepted. Specified in [SPEC-464](../../.ai/specs/SPEC-464-we-still-have-healthy-subscribtionsbut-w.md);
task #449, issue #1172. Refines [ADR-0225](0225-claude-code-owns-the-goal-retry-after-an-api-error.md)
(which goal pauses need a person) and SPEC-001 ROT-009 (what a usage reading replaces).

## Context

Fable 5.2 became the default model on 2026-11-10. It runs on a weekly allowance of usage credits
that Claude counts beside a subscription's 5-hour and weekly windows (`seven_day_overage_included`,
which Claude Code calls the "Fable limit"). On 2026-11-11 a subscription's allowance ran out
while its week stood at 61 %. Claude Code answered "You're out of usage credits. /model to switch
models." and paused the goal ("Goal paused · the request was rate limited"). Three things went
wrong:

- Claude Code 2.1.311 writes the refusal as `apiError: "model_requires_usage_credits"` without
  `rate_limit_info`. The harness knew only `usage_limit_reached` and a `rejected` status, so it
  read nothing: three tasks stayed on the subscription until a person moved them.
- The goal pause was read as one nobody handles (ADR-0225): eight tasks asked for "continue",
  among them the tasks Yulius had already moved (their sessions ran 2.1.322, whose record carries
  `rate_limit_info`).
- The limit was stored as the subscription's week, and the next usage reading replaced the whole
  stored usage. Minutes later the subscription read `ok` and the pool placed a task back onto it.
  No reading shows the allowance: Claude Code's status line and the probe's response headers
  carry the 5-hour and weekly windows only.

## Decision

- **The refusal is a usage limit.** The harness reads `model_requires_usage_credits` as
  `LimitReached`, with or without `rate_limit_info`, and a record that has only the words the same
  way. The limit path (ADR-0027, ADR-0058) then does what it does for every limit: the task moves
  past the cut turn or waits and resumes at the reset.
- **A limit says what it binds** (`LimitScope`): the subscription (`five_hour`, `seven_day`, an
  unknown kind) or the models of one family. A per-model cap names its family (`seven_day_opus`,
  `seven_day_sonnet`). The allowance of usage credits (`seven_day_overage_included`, a bucket
  per model) and a credits refusal that names no window are the limit of the model the task is
  launched with: the default model when the task names none. The family is never hardcoded, so
  the limit always binds the task that hit it.
- **A model's limit is stored on that model's weekly window.** `ModelFamily::Fable` gets one
  (`AccountUsage::weekly_fable`, beside `weekly_opus` and `weekly_sonnet`), full until the reset
  Claude named. It binds only tasks of that model; the subscription's own windows keep their
  readings. The allowance's reset is taken only from its own window: when the response claimed
  another window, that window's reset is not when the model runs again.
- **A reading keeps what it cannot see.** A provider reading replaces the windows it carries and
  keeps the per-model windows it does not carry while their reset is ahead. A per-model window
  whose reset passed is dropped at the next stored reading: nothing is known about the new one.
  A new sign-in of the subscription drops the per-model windows: they are what Claude told the
  previous sign-in, which may have been another Claude account.
- **A model's limit without a reset stays a limit of the subscription**, with no reset, as every
  limit without one was: the next usage reading ends it.
- **A subscription reads `limited` while the default model cannot run on it**: a limit Claude
  reported is in effect, or a window that binds that model is at 100 %. The capacity view counts
  subscriptions for that model.
- **A goal pause at a limit is the limit's.** A pause notice that follows a usage limit or a
  refused subscription in the same turn opens no Needs you and writes no
  `task.goal_retries_spent` while Yulius takes the task on, live (`goal_retry_observed`, by the
  run's limit mark) and in the replay (`TranscriptFacts::fold`, by the pending limit). A pause
  with no limit before it still needs a person.
- **The pause is held, not dropped** (`LimitGoalPause`). When the switch past the limit is
  refused before it ends the session (ROT-014), the session stays at Claude Code's refusal and
  nothing goes on by itself, so the held pause opens the Needs you after all, once, whichever of
  the notice and the refusal came first. A later move past the limit continues the cut turn and
  ends that request. A task already under Needs you is told nothing new (a restart reads the
  same pause again); one that a message set working since, refused again, is under Needs you
  again.

## Alternatives

- **Switch the task to another model** (what Claude Code suggests with `/model`). The person
  chose the model, and another subscription can run it. Rejected.
- **Keep an account-wide limit that readings may not clear.** It would keep every model off the
  subscription, show a week at 100 % that is at 61 %, and stay after Claude reset a 5-hour or
  weekly window early, which a reading does show. Rejected.
- **Map `seven_day_overage_included` to Fable in the harness**, as Claude Code's label "Fable
  limit" does. A task of another model refused for its own bucket would not be bound by the
  stored limit and would be sent back to the same subscription within seconds. Rejected.
- **Drop the goal pause in the harness by its wording.** "The request was rate limited" also
  follows refusals nobody else handles (a throttle Claude Code gave up on), and those need a
  person. The decision belongs where the limit is known. Rejected.
- **Drop the pause for good once a limit was seen.** A switch refused in preflight leaves the
  session at the refusal with only a line in its terminal, and Claude Code does not continue a
  credits refusal at a reset as it does a window's limit. Rejected.
- **A fixed back-off for a model's limit without a reset.** It needs a time per limit in the
  stored usage. Only sessions still on Claude Code 2.1.311 and the "credits are off" refusal lack
  the reset, and a retry costs one refused request and a move. Not now.
- **Read the allowance ahead of the limit.** Claude sends it in
  `anthropic-ratelimit-unified-7d_oi-*` headers on the model's own responses, so the probe would
  have to ask with that model. Left to the Subscriptions page's manual refresh (a chained task),
  which needs the same request to re-read a limit a person lifted.

## Consequences

- A subscription out of a model's credits loses that model's tasks at once and gets none until
  the allowance resets; tasks of other models still run on it.
- Per-model caps reported as a limit (`seven_day_opus`, `seven_day_sonnet`) no longer mark the
  whole week full, and they survive a reading from response headers, which never carried them.
- The stored usage (`accounts.usage_json`) and the API's `usage` gain `weekly_fable`; older rows
  read without one. No migration.
- A limit a person lifts before its reset (bought credits) stays until the reset, or until the
  subscription is signed in again: there is no way yet to ask Claude again. The chained task
  adds one.
- A subscription on which the model can never run (credits off, no allowance) is tried again
  after each usage reading, about every five minutes while tasks need a place.
- The match follows Claude Code 2.1.311 and 2.1.322. A CLI that rewords the refusal and drops its
  typed kind falls back to the goal pause's Needs you, never to a task that moves for no reason.
