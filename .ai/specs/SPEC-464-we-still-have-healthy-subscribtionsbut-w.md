---
yulius-task: bf4157c9-9530-4f30-b545-d23684dc1641
status: in-progress
---

# SPEC-464: a subscription out of usage credits for its model is a usage limit

Status: in progress
Date: 2026-11-11
Owner: Yulius task #449
Scope authorization: the person's request of 2026-11-11 ("we still have healthy subscriptions but
we're getting errors. Fix.", two screenshots) and their follow-up of the same day with production
records and two more requirements (a manual refresh on the Subscriptions page; reset times known
and used). Issue [#1172](https://github.com/Quartio-Studio/yulius/issues/1172).

## Goal

A task whose subscription refuses its model for used-up usage credits goes on by itself on a
subscription that can run it, or waits and resumes at the reset Claude named. The pool keeps away
from that subscription for that model until the reset, whatever a usage reading says, and nobody
is asked to send "continue".

## Context

Symptom (production, 2026-11-11 14:49–15:27 UTC). Since Fable 5.2 is the default model
(2026-11-10), tasks stopped with "You're out of usage credits. /model to switch models.", then
"Goal paused · the request was rate limited", and a Needs you asking to send "continue". Sending
it failed the same way. Other subscriptions had capacity the whole time.

Root cause, three defects in one path:

1. **The refusal is not read as a limit.** Fable runs on a weekly allowance of usage credits
   (Claude's `seven_day_overage_included` window, "Fable limit") beside the 5-hour and weekly
   windows. When it is used up and no extra credits are left, Claude Code writes an API error
   record with `error: "rate_limit"`, `apiError: "model_requires_usage_credits"`, status 429.
   Claude Code 2.1.311 writes it without `rate_limit_info`.
   `yulius_harness::claude::observe_line` returns nothing for that line: `structured_limit` wants
   `apiError: "usage_limit_reached"` or a `rejected` status, and the text matches no limit
   phrase. Tasks #438, #142 and #154 (sessions started on 2.1.311) were never moved. Claude Code
   2.1.322 adds `rate_limit_info` (`status: rejected`, `resetsAt`), so #156, #443, #149 and #151
   were moved in the same second. The existing tests held only `usage_limit_reached` records.
2. **The goal pause that follows opens a Needs you.** Claude Code's goal mode answers this
   refusal with "Goal paused · the request was rate limited · send a message to retry"
   ([ADR-0225](../../docs/decisions/0225-claude-code-owns-the-goal-retry-after-an-api-error.md)
   reads it as a pause nobody handles). `task.goal_retries_spent` was audited 11 times for 8
   tasks, also for the tasks Yulius had already moved. ADR-0225 left the usage limit to the limit
   path, but only the notice "usage limit reached" was excluded, and Claude Code does not use
   that wording here.
3. **A usage reading wipes the limit.** `record_limit` stored the limit as the subscription's
   week ("This week 100% used", although the week was at 61 %) plus `limit_reached`. The next
   probe reading replaces the whole stored usage (`fetch_and_record_usage`), so the subscription
   read `ok` again with `limit_reached: false`. At 15:20:57 the pool moved #159 onto it
   ("Selected { tier: Roomy }"); it was refused two seconds later. No reading shows the model's
   allowance: Claude Code's status line and the probe's headers carry only the 5-hour and weekly
   windows.

What exists and is reused: the limit path (SPEC-034, SPEC-072, SPEC-074: `record_limit`,
`rotate` with `Trigger::LimitReached`, the transcript step back, the `limit` pause and the resume
loop that wakes at the earliest known reset), the per-model weekly caps (`weekly_opus`,
`weekly_sonnet`, `ModelFamily`, `binding_windows`) and the Superagent's move (SPEC-074).

What Claude Code documents about the record (its own schema text, 2.1.322):
`model_requires_usage_credits`: "the model runs on usage credits for this account, and credits are
off or the included allowance is used up"; its `rate_limit_info` has `status: rejected`,
`rateLimitType` `seven_day_overage_included` "when an included allowance was used up" and absent
"when it named none", and `resetsAt` "when the response had them".

Parallel work: PR #1167 (task #443) touches `observe.rs`, `ports/harness.rs`, `wiring/harness.rs`,
`finish.rs` and `supervisor.rs` for another purpose; `dev` is merged before the final review.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| UCL-001 | `observe_line` reads an API error record with `apiError: "model_requires_usage_credits"` as `LimitReached`, with or without `rate_limit_info`. Its reset is `resetsAt` only when `rateLimitType` is `seven_day_overage_included` (another claim's reset is not the allowance's). A record with only the text ("out of usage credits", "requires usage credits", "out of extra usage") is read the same way. The transcript step back removes the refusal, the goal's pause notice and the record closing that turn. | `claude::tests::a_usage_credits_refusal_is_a_usage_limit` (the two recorded lines, ids replaced), `step_back::tests::step_back_removes_a_usage_credits_refusal` |
| UCL-002 | A limit says what it binds (`LimitScope`): the subscription (`five_hour`, `seven_day`, an unknown kind), or one model family: the one a per-model cap names (`seven_day_opus`, `seven_day_sonnet`), or the family of the model the task is launched with, for the allowance of usage credits (`seven_day_overage_included`, a bucket per model) and for a credits refusal that names no window. A task that names no model runs the default one. | same test; `structured_limit_record_with_reset_time`; `capacity::tests::a_tasks_model_family_is_its_models_or_the_defaults` |
| UCL-003 | `ModelFamily::Fable` (a model name containing "fable") has its own weekly window, `AccountUsage::weekly_fable`, which binds only Fable tasks. A model's limit with a known reset is stored as that family's window at 100 % until the reset; the 5-hour and weekly windows and `limit_reached` are left as they are. A model's limit without a reset, or for a family without a window, is stored as before (`limit_reached`, no reset: in effect until the next usage reading). | `usage::tests::a_models_limit_is_its_own_window`, `rotation::tests::a_fable_allowance_binds_only_fable_tasks` |
| UCL-004 | A usage reading from the provider replaces the windows it carries and keeps the per-model windows it does not carry while their reset is ahead; a per-model window whose reset passed is dropped at the next stored reading. A new sign-in of the subscription drops the per-model windows: they are what Claude told the previous sign-in, and no reading would end them. A limit Claude reports while a reading of the same subscription is under way is not written over: a reported limit, a session's reading, a provider's reading and a new sign-in each read, change and write the stored usage under one lock (`CapacityState::usage_writes`), never held across a request to the provider. | `usage::tests::a_reading_keeps_the_model_windows_it_cannot_see`; `usage_credits::a_subscription_out_of_usage_credits_gets_no_task_until_they_reset`; `account_email::a_new_sign_in_drops_the_models_limit_of_the_previous_one`; `usage_credits::a_limit_reported_during_a_usage_reading_is_kept` |
| UCL-005 | A subscription reads `limited` while the default model cannot run on it: a limit Claude reported is in effect or a window binding the default model is at 100 %. The capacity view's "available" counts subscriptions for the default model. | `usage_credits::a_subscription_out_of_usage_credits_gets_no_task_until_they_reset` (status after the limit and after the reading); `account_usage::a_reading_keeps_a_models_limit_and_the_status_follows_the_windows` |
| UCL-006 | A task refused for usage credits moves to a subscription that can run its model, in the same session, past the refused turn (transcript stepped back, nothing typed). With no such subscription it waits in a `limit` pause and resumes at the reset by itself, with no usage reading and no message. The pool places no task of that model on the subscription before the reset. | `usage_credits::a_task_out_of_usage_credits_moves_on_by_itself`, `…gets_no_task_until_they_reset`, `…resumes_at_their_reset_by_itself` |
| UCL-007 | A goal pause Claude Code makes at a usage limit or a refused subscription (its notice follows the limit record of the same turn) opens no Needs you and writes no `task.goal_retries_spent` while Yulius takes the task on (moves it or pauses it), live and in the replay after a restart. Its notice is held: when the switch past the limit is refused before it ends the session (ROT-014), the session stays at the refusal and nothing goes on by itself, so the pause opens the Needs you after all, whichever of the two came first; a later move past the limit ends that request by itself. It is asked and audited once per limit: while the request made for a limit is open (asked at or after Claude Code recorded that limit), reading the same pause again (a restart replays it) only puts the task back under Needs you, whatever it read meanwhile; a refusal after a later prompt (Yulius's own, which ends no request) is a new limit, audited again, and the open request is kept. A goal pause with no limit before it is unchanged (ADR-0225). | `usage_credits::a_task_out_of_usage_credits_moves_on_by_itself`, `…gets_no_task_until_they_reset`, `…resumes_at_their_reset_by_itself` (`needs_nobody`); `usage_credits::a_refused_move_at_a_usage_credits_limit_still_asks_a_person`, `usage_credits::a_usage_credits_refusal_written_while_yulius_was_down_is_read_at_restart`; `finish::tests::a_goal_pause_at_a_usage_limit_is_the_limits`, `goal_retry::tests::a_goal_pause_at_a_limit_asks_a_person_only_when_the_move_is_refused`; `task_done.rs` goal-retry tests unchanged |
| UCL-008 | The Superagent's limit path takes the same record: it records the limit with what it binds and moves as for any usage limit. | `cockpit::a_superagent_out_of_usage_credits_moves_to_another_subscription` |
| UCL-009 | `GET /api/admin/accounts`, `/api/status` and `/api/capacity` carry `usage.weekly_fable` (one `usage_view`); the subscription card shows it as "Fable this week", like the Opus and Sonnet rows. | `account_usage::a_reading_keeps_a_models_limit_and_the_status_follows_the_windows`, `accounts::tests::unknown_usage_stays_null`; e2e `subscriptions.e2e.ts` "a subscription out of its model's usage credits shows that model's week beside its own" |

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Spec, issue #1172 and the regression tests that fail
- [x] The harness reads the usage-credits refusal as a usage limit with what it binds
- [x] A model's limit is stored on its own weekly window and binds only that model's tasks
- [x] A usage reading keeps the limits it cannot see and the subscription reads Limit reached
- [x] A goal pause at a usage limit opens no Needs you, live and after a restart
- [x] The Subscriptions card and the API show the model's weekly window
- [x] Docs, ADR-0301, changes and the chained task for the page's manual refresh
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green on the reviewed head

## Acceptance criteria

- The two recorded lines (2.1.311 without `rate_limit_info`, 2.1.322 with `resetsAt`) are read as
  a usage limit of the model; the 2.1.322 one carries the reset.
- A task refused this way runs on another subscription within the same session, its transcript
  holds no refusal record, nothing was typed and no Needs you is open.
- After the refusal the subscription's stored usage has `weekly_fable` at 100 % with the reset
  Claude named and its week unchanged; after a usage reading with the week at 61 % both are still
  there, the status is `limited`, and a new task starts on the other subscription.
- With one subscription the task waits in a `limit` pause and runs again within seconds of the
  reset, with no reading and no message.
- A goal pause "the request was rate limited" with no limit record before it still opens Needs
  you.
- When the move past the limit is refused (the other subscription's credential cannot be
  opened), the task stays on its subscription with the Needs you "Goal paused · the request was
  rate limited."; a person's move to a working subscription then continues the turn and ends
  that request.

## Test plan

- Fake agent: `--credits-out` writes the 2.1.311 record, the goal notice and the record closing
  the turn; `--credits-reset-s S` writes the 2.1.322 record whose allowance resets S seconds
  ahead.
- Unit (`yulius-harness`): the two recorded lines; a text-only record; per-model `rateLimitType`
  values; a `usage_limit_reached` record stays the subscription's.
- Unit (`yulius-domain`): a model's limit is its own window; Fable binds only Fable tasks; a
  reading keeps the model windows it cannot see and drops them after their reset; rotation places
  a Fable task off a subscription whose Fable window is full and an Opus task on it.
- Unit (`yulius-app`): the replay drops a goal pause that follows a limit; `usage_json` round
  trip with `weekly_fable`.
- Integration (`usage_credits.rs`, real ptyhost and fake agent): the three scenarios of UCL-006,
  each asserting no Needs you. They fail before the fix: the 2.1.311 one never moves, the
  2.1.322 ones open the Needs you and store the week as full. Two more for UCL-007: a move
  refused in preflight opens the Needs you, a further refused message audits again and keeps the
  request, and a person's move ends it; a refusal written while Yulius was down is read by the
  restart's replay, and a further restart asks nothing new. That refusal is appended to the
  session's transcript while no server runs (task #438's records), so no timing decides who
  saw it. One for the lock of UCL-004: a reading is held after it read the stored usage
  (test-only pause points `UsageRead` and `LimitRecord`), the refusal arrives, the reading goes
  on, and both the limit and the reading's windows are stored.
- Integration, further: a limit named for the allowance binds the model of the task that hit it
  (an Opus task: `weekly_opus`, the subscription still takes a default-model task); a new sign-in
  drops the previous sign-in's model limit (`account_email.rs`); the Superagent (`cockpit.rs`).
- Existing suites that cover the paths touched: `pool_supervision`, `subscription_moves`,
  `subscription_pool`, `account_usage`, `accounts`, `cockpit`, `task_done`, `ui_contract`.
- e2e (`cd e2e && npx e2e run`): the Subscriptions page shows the Fable row.

## Decisions

- **The refusal is a usage limit of the model, not a third kind of event** (ADR-0301). The limit
  path already does everything the person asked for: it moves the task past the cut turn, pauses
  without a message and resumes at the reset. Rejected: typing `/model` to switch the task to
  another model (the person chose the model; other subscriptions can run it), and retrying on
  the same subscription.
- **What a limit binds travels with it** (`LimitScope`), and a model's limit is stored on that
  model's weekly window. `record_limit` used to mark the subscription's week full for every
  `seven_day_*` type, which also kept Opus and Sonnet tasks off it and showed a wrong 100 %.
  Rejected: an account-wide limit with a reset that readings may not clear (it would keep a
  subscription out after Claude reset a 5-hour or weekly window early, which a reading does
  show), and a free-text "kind" beside the limit (the page and the policy would each interpret
  it).
- **The allowance's limit is the limit of the model the task runs, not a hardcoded family.**
  Claude Code describes `seven_day_overage_included` as a bucket per model and labels it "Fable
  limit" after the one model that has it today. Yulius stores it on the family of the model the
  task is launched with (Fable for the default model). So the limit always binds the task that
  hit it: a limit that did not would send the task back to the same subscription at once.
  A family without a window of its own (Haiku, unknown names) gets a limit of the whole
  subscription until the reset, which the next usage reading ends. Assumed (reversible): both
  Fable models share one family; a model switched inside the session with `/model` is not
  followed, since the next launch runs the task's model again.
- **A reading keeps what it cannot see.** Per-model windows a reading does not carry stay until
  their reset; the session readings already worked that way, the probe did not. A person who
  bought credits clears the limit with the page's refresh (the chained task below).
- **A new sign-in drops them.** A subscription signed in again may be another Claude account,
  and the windows it cannot read would otherwise hold for up to a week. If the same account is
  still out of credits, its next refusal records the limit again, at the cost of one refused
  request. The 5-hour and weekly readings stay: the next reading replaces them.
- **A model's limit without a reset stays the old account-wide limit**, in effect until the next
  usage reading (about five minutes). Only records of sessions still on Claude Code 2.1.311 and
  the "credits are off" refusal lack the reset. Known limit: a subscription on which the model
  can never run is tried again after each reading; the try costs one refused request and a move.
  Rejected for now: a fixed back-off (it needs a timestamp per limit in the stored usage).
- **A goal pause at a limit is the limit's** (refines ADR-0225). Decided where the pause is
  acted on (`goal_retry_observed`, `TranscriptFacts::fold`), by the limit the same turn recorded,
  not by wording: the notice "the request was rate limited" also follows refusals nobody else
  handles, and those still need a person.
- **The pause is held, not dropped** (`LimitGoalPause`). A switch past the limit can be refused
  before it ends the session (another subscription's credential cannot be opened, ROT-014); the
  session then sits at Claude Code's refusal, and unlike a usage limit of a window Claude Code
  does not continue it at a reset. The held pause asks a person then. The move runs beside the
  transcript's reading, so the notice and the refusal leave their word for each other in one
  lock. Rejected: dropping the pause (a task nobody is told about), and a `limit` pause for a
  refused switch (ADR-0058 rejected it: the resume loop would retry a refused switch every
  round).
- **One lock for the stored usage, in the app.** Four paths read a subscription's usage, change
  it and write it back, and the store has one JSON column for it. A reading that read before a
  limit was recorded wrote over it: the same loss as on 2026-11-11, in a shorter window. The lock
  is a `tokio` mutex in `CapacityState`, taken after the provider answered. Rejected: a
  compare-and-set in the store (a new store contract for one JSON column), and merging in SQL.
- **Asked once per limit, by the request's time.** The runtime keeps when Claude Code recorded
  the limit that cut the turn; a request asked at or after it is that limit's. The first version
  skipped a task that already read Needs you, but the replay settles a closed turn as Your turn
  beside the move's refusal, and whichever lands last decided whether a later restart asked
  again.
- **Known limit: the capacity card's "Left this week"** still sums the subscriptions' own weeks;
  only "Available now" counts for the default model. A subscription out of Fable's allowance
  keeps its week's headroom in that figure.
- **Split by concern.** The person's first extra requirement (a manual refresh per subscription
  and for all, each card saying how old its reading is and which window is used up) is a page and
  probe feature of its own: it needs a probe request on the limited model to re-read "limit state
  now" (Claude's `anthropic-ratelimit-unified-7d_oi-*` headers) and a state computed when the page
  is shown. It is task #451, chained after this one, with its brief. Their second requirement (reset
  times known and used, tasks resume at the reset by themselves) is this change: UCL-003, UCL-004
  and UCL-006.

## Evidence

Reproduction (read-only, production records of 2026-11-11, UTC):

- Task #438's transcript (Claude Code 2.1.311): at 14:49:49.800 an `assistant` record with
  `isApiErrorMessage`, `error: "rate_limit"`, `apiError: "model_requires_usage_credits"`, status
  429, no `apiErrorParams`, text "You're out of usage credits. /model to switch models."; 4 ms
  later the notice "Goal paused · the request was rate limited · send a message to retry", then
  `turn_duration`. The same three records follow each "continue" (15:05:52, 15:17:36, 15:17:48,
  15:21:05).
- Task #443's sub-agent transcript (2.1.322): the same record with
  `rate_limit_info {status: rejected, resetsAt: 1794556800, rateLimitType:
  seven_day_overage_included, overageDisabledReason: out_of_credits}`.
- Claude Code 2.1.322's own schema text and code (strings of the installed binary): the typed
  kind, when `rate_limit_info` carries the claim, the message list, and the status line's
  `rate_limits` (5-hour and weekly only).

Regression tests first (commit `3326040a`, before any fix):

- `cargo test -p yulius-harness --lib a_usage_credits_refusal`: FAILED, `observe_line` returned
  `[]` for the 2.1.311 line.
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^usage_credits::/)'`: 0 of 3
  passed. The 2.1.311 task never moved ("timed out waiting for the next run", 60 s); after the
  2.1.322 record the Needs you "Send the task a message (for example "continue") …" was open.
- Later behaviours, each red before its change: the refused move ("timed out waiting for the
  request"), the new sign-in (the stored `weekly_fable` and `weekly_opus` were still there), the
  repeat at a refused move ("timed out waiting for the second refusal").

After the fix:

- `usage_credits::` 6 of 6 pass; `account_usage::`, `account_email::`, `accounts::` 24 pass;
  `cockpit::a_superagent_out_of_usage_credits_moves_to_another_subscription` passes.
- Mutation checks (the fix reverted in one place at a time, each failed by the tests named in
  Requirements): the credits kind and phrases; the allowance named as Fable; the task's model
  family; the model's window kept across a reading; the status rule; the goal pause held at a
  limit, live and in the replay; the replay's call; the refusal's call; the request ended by a
  move; the hold after a refusal; the pause kept in the fold; the sign-in's drop; asked once at a
  restart. 16 mutations, 16 caught.
- `scripts/dev ci` on `fddcdb8c` (with `dev` merged): green. 3508 tests run, 3508 passed, 16
  skipped; repository contracts, check, fmt, clippy (Linux and macOS), doctests, deny, shear and
  docs passed. An earlier run failed only at the docs stage (a doc link to a private item),
  fixed in `8110f414`.
- `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run`: 77 files passed, 1 skipped; 923 tests
  passed, 1 skipped. `tests/subscriptions.e2e.ts`: 20 of 20, with the new test.
- The mock's Subscriptions page looked at in the sandbox browser: "Claude #5" reads "Limit
  reached" with "Fable this week 100% used, Resets Friday …" under its own 5-hour and weekly rows.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius, for this
  project or the whole instance; nothing was scanned." Disabled, not passed.

Not tested here:

- A real Claude Code session on a subscription out of usage credits: the fake agent writes the
  two recorded shapes.
- The refusal when Claude's response claims another window (`errorCode: credits_required`) and
  the text-only wordings: synthetic records built from Claude Code 2.1.322's code.
- The order "move refused first, pause notice second" with real processes: the fake agent writes
  both lines at once, so the notice is read first. That order is covered by the unit test of
  `LimitGoalPause`.
