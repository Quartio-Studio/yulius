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
  `LimitReached`, with or without `rate_limit_info`. A record that has only the words is read
  too: the model's when the text names the model or a way to another one ("Fable 5 requires
  usage credits", "/model to switch models"), else the subscription's, since Claude Code uses
  the same words for a subscription whose window and credits are both used up ("You're out of
  usage credits · resets 3pm"). The limit path (ADR-0027, ADR-0058) then does what it does for
  every limit: the task moves past the cut turn or waits and resumes at the reset.
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
  another window, that window's reset is not when the model runs again. Without a reset the
  window is full with no reset time, still only that model's.
- **A reset is believed only as far as a window lasts**: a week and a day ahead
  (`LIMIT_RESET_MAX_MS`). The limit record comes from the session's transcript, which is the
  agent's own file, and a model's window is kept across readings. A reset further ahead is no
  reset, for the subscription's own limits too
  ([SPEC-398](../../.ai/specs/SPEC-398-vidoc-triage-supervisor-harness-and-host.md) VSH-005
  bounds a session's reading the same way).
- **A reading keeps what it cannot see.** A provider reading replaces the windows it carries (the
  usage endpoint carries the Opus and Sonnet caps when the plan has them; nothing carries the
  allowance) and keeps the per-model windows it does not carry while their reset is ahead. It
  keeps none without a reset, so a provider's reading ends a model's limit nothing says the end
  of. A session's reading replaces the 5-hour and weekly windows only. A per-model window whose
  reset passed is dropped at the next stored reading. A new sign-in of the subscription drops
  the per-model windows: they are what Claude told the previous sign-in, which may have been
  another Claude account.
- **One lock for the stored usage.** A reported limit, a session's reading and a provider's
  reading read, change and store it through one helper (`App::change_account_usage`) under
  `CapacityState::usage_writes`, which a new sign-in takes too. It is taken after the provider
  answered. A limit recorded while a reading is under way is added to what the reading stored,
  not written over.
- **A subscription reads `limited` while the default model cannot run on it**: a limit Claude
  reported is in effect, or a window that binds that model is at 100 %. The capacity view counts
  subscriptions for that model. "Run #N on…" judges each subscription by what binds the task's
  own model, as the server does: the API's usage says whether a limit of the whole subscription
  is in effect (`limit_reached`, `limit_resets_at`), and the menu reads the task's model itself
  when its caller does not have it yet.
- **One blind retry, then no more by itself.** A model's limit nothing says the end of, with no
  subscription the task can go to, makes the task wait; a provider's reading ends such a limit
  and the pool tries the task once (`BLIND_LIMIT_RETRIES`). Refused the same way again before
  the model answered, the task is not started on that subscription by itself again. When another
  subscription says when it resets, the task waits for that reset, and the pool leaves the
  refusing subscription out for this task whatever its readings say
  (`rotation::decide_leaving_out`). When nothing says when the task can go on, its session stays
  at the refusal, and Yulius asks a person in the terminal and under Needs you
  (`task.limit_without_end`); further readings start nothing, and the request ends when the task
  goes on past the limit. The bound is for a model's limit: a limit of the subscription ends by
  what a reading shows of its windows.
- **A goal pause at a limit is the limit's.** A pause notice that follows a usage limit or a
  refused subscription in the same turn opens no Needs you and writes no
  `task.goal_retries_spent` while Yulius takes the task on, live (`goal_retry_observed`, by the
  run's limit mark) and in the replay (`TranscriptFacts::fold`, by the pending limit). A pause
  with no limit before it still needs a person.
- **The pause is held, not dropped** (`LimitGoalPause`). When the switch past the limit is
  refused before it ends the session (ROT-014), the session stays at Claude Code's refusal and
  nothing goes on by itself, so the held pause opens the Needs you after all, whichever of the
  notice and the refusal came first. A later move past the limit continues the cut turn and
  ends that request. The pause is asked and audited once per limit, and the audit is the record
  of that: the runtime keeps when Claude Code recorded the limit, and an entry written at or
  after it says the limit was asked about. Reading the same pause again (a restart replays it)
  then only puts the task back under Needs you; a refusal after a later prompt is a new limit.
  When Yulius asks a person about the limit itself (a limit nothing says the end of), the pause
  written there adds nothing and is not asked about, read before or after (`Asked`).

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
- **Go by the open request to say a limit was asked about.** By the task reading Needs you: the
  replay settles the closed turn beside the refused move, and whichever lands last decided. By
  the request's time: the request kept from an earlier limit is older than a later one, so each
  restart audited the later one again. Rejected for the audit entry.
- **A compare-and-set in the store for the usage column**, or merging the windows in SQL. A new
  store contract for one JSON column; the writers are all in the app. Rejected for the lock.
- **Limit the whole subscription when a model's limit has no reset** (the first version). Other
  models' tasks were moved off it at every such refusal. Rejected for the model's window.
- **An assumed reset for a limit nothing says the end of** (an hour ahead, kept across
  readings). It bounds the retries too, but shows a made-up time as "Resets at" on the card and
  in the pause, and still never tells anyone. Rejected for the retry count.
- **Ask a person whenever the retry is refused**, also when another subscription says when it
  resets. The task would sit at its refusal past that reset until someone moved it, where the
  pool can take it on by itself. Rejected: it waits for the known reset.
- **Keep the pause and let the pool decide as for any paused task** (round 2's first version of
  that case). Each reading of the refusing subscription ended its limit, so the session was
  started there and refused again, every few minutes until the other one's reset. Rejected for
  leaving the subscription out for that task.
- **Tell "used up" from "switched off" by Claude's `overageDisabledReason`.** Claude Code 2.1.311
  records carry none, and a refusal whose body alone says credits are required names no window.
  Left to the probe of task #451, which reads the model's state itself.
- **Read the allowance ahead of the limit.** Claude sends it in
  `anthropic-ratelimit-unified-7d_oi-*` headers on the model's own responses, so the probe would
  have to ask with that model. Left to the Subscriptions page's manual refresh (task #451),
  which needs the same request to re-read a limit a person lifted.

## Consequences

- A subscription out of a model's credits loses that model's tasks at once and gets none until
  the allowance resets; tasks of other models still run on it.
- Per-model caps reported as a limit (`seven_day_opus`, `seven_day_sonnet`) no longer mark the
  whole week full, and they survive a reading from response headers, which never carried them.
- The stored usage (`accounts.usage`) and the API's `usage` gain `weekly_fable`; older rows read
  without one. No migration.
- A limit a person lifts before its reset (bought credits) stays until the reset, or until the
  subscription is signed in again: there is no way yet to ask Claude again. Task #451 adds one.
- A limit without a reset ends with the next provider reading of its subscription. A round skips
  a subscription whose sessions reported usage in the last 90 s, so on a busy subscription that
  can take a while.
- A subscription on which a model can never run (credits switched off) and that says no reset
  gets one refused request from each task of that model placed on it after a provider reading:
  the task moves on, or with nowhere to go is tried once more and then waits for a known reset
  elsewhere or asks a person. What a task remembers of such limits is in memory: a restart of
  Yulius allows one more retry.
- The reset's bound leaves this: a record forged in a session's transcript with a reset up to
  eight days ahead keeps that model off the subscription until then, across readings, and moves
  the task that "hit" it to the next subscription. A new sign-in of the subscription ends it;
  task #451's refresh will re-read the model's state.
- A refusal Claude Code writes no goal notice for (a queued check-in while the goal is paused
  already) has no held pause: a move refused there leaves the task reading Working at its
  refusal, the earlier request still open, as every limit without a goal notice did before.
- The match follows Claude Code 2.1.311 and 2.1.322. A CLI that rewords the refusal and drops its
  typed kind falls back to the goal pause's Needs you, never to a task that moves for no reason.
