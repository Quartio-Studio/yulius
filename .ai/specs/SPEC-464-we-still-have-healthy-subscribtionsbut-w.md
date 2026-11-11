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
| UCL-001 | `observe_line` reads an API error record with `apiError: "model_requires_usage_credits"` as `LimitReached`, with or without `rate_limit_info`. Its reset is `resetsAt` only when `rateLimitType` is `seven_day_overage_included` (another claim's reset is not the allowance's). A record with only the text ("out of usage credits", "requires usage credits", "out of extra usage") is read the same way. | `claude::tests::a_usage_credits_refusal_is_a_usage_limit` (the two recorded lines, ids replaced) |
| UCL-002 | A limit says what it binds (`LimitScope`): the subscription (`five_hour`, `seven_day`, an unknown kind), or one model family (`seven_day_opus`, `seven_day_sonnet`, `seven_day_overage_included` = Fable; a credits refusal that names no window = the model the session runs). | same test; `structured_limit_record_with_reset_time` |
| UCL-003 | `ModelFamily::Fable` (a model name containing "fable") has its own weekly window, `AccountUsage::weekly_fable`, which binds only Fable tasks. A model's limit with a known reset is stored as that family's window at 100 % until the reset; the 5-hour and weekly windows and `limit_reached` are left as they are. A model's limit without a reset, or for a family without a window, is stored as before (`limit_reached`, no reset: in effect until the next usage reading). | `usage::tests::a_models_limit_is_its_own_window`, `rotation::tests::a_fable_allowance_binds_only_fable_tasks` |
| UCL-004 | A usage reading from the provider replaces the windows it carries and keeps the per-model windows it does not carry while their reset is ahead; a per-model window whose reset passed is dropped at the next stored reading. | `usage::tests::a_reading_keeps_the_model_windows_it_cannot_see`; `usage_credits::a_subscription_out_of_usage_credits_gets_no_task_until_they_reset` |
| UCL-005 | A subscription reads `limited` while the default model cannot run on it: a limit Claude reported is in effect or a window binding the default model is at 100 %. The capacity view's "available" counts subscriptions for the default model. | `usage_credits` (status after the limit and after the reading); `account_usage.rs` |
| UCL-006 | A task refused for usage credits moves to a subscription that can run its model, in the same session, past the refused turn (transcript stepped back, nothing typed). With no such subscription it waits in a `limit` pause and resumes at the reset by itself, with no usage reading and no message. The pool places no task of that model on the subscription before the reset. | `usage_credits::a_task_out_of_usage_credits_moves_on_by_itself`, `…gets_no_task_until_they_reset`, `…resumes_at_their_reset_by_itself` |
| UCL-007 | A goal pause Claude Code makes at a usage limit or a refused subscription (its notice follows the limit record of the same turn) opens no Needs you and writes no `task.goal_retries_spent`, live and in the replay after a restart. A goal pause with no limit before it is unchanged (ADR-0225). | the three `usage_credits` tests (`needs_nobody`); `finish::tests::a_goal_pause_at_a_usage_limit_is_the_limits`; `task_done.rs` goal-retry tests unchanged |
| UCL-008 | The Superagent's limit path takes the same record: it records the limit with what it binds and moves as for any usage limit. | `cockpit.rs` |
| UCL-009 | `GET /api/admin/accounts`, `/api/status` and `/api/capacity` carry `usage.weekly_fable`; the subscription card shows it as "Fable this week", like the Opus and Sonnet rows. | `ui_contract.rs`, e2e `subscriptions` |

## Plan

- [x] Reproduce the defect and find its root cause
- [ ] Spec, issue #1172 and the regression tests that fail
- [ ] The harness reads the usage-credits refusal as a usage limit with what it binds
- [ ] A model's limit is stored on its own weekly window and binds only that model's tasks
- [ ] A usage reading keeps the limits it cannot see and the subscription reads Limit reached
- [ ] A goal pause at a usage limit opens no Needs you, live and after a restart
- [ ] The Subscriptions card and the API show the model's weekly window
- [ ] Docs, ADR-0301, changes and the chained task for the page's manual refresh
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
  2.1.322 ones open the Needs you and store the week as full.
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
- **`seven_day_overage_included` is the Fable window.** Claude Code names it "Fable limit" and
  describes it as a per-model bucket. Assumed (reversible): Mythos names are not mapped to it;
  such a task's refusal falls back to the account-wide limit.
- **A reading keeps what it cannot see.** Per-model windows a reading does not carry stay until
  their reset; the session readings already worked that way, the probe did not. A person who
  bought credits clears the limit with the page's refresh (the chained task below).
- **A model's limit without a reset stays the old account-wide limit**, in effect until the next
  usage reading (about five minutes). Only records of sessions still on Claude Code 2.1.311 and
  the "credits are off" refusal lack the reset. Known limit: a subscription on which the model
  can never run is tried again after each reading; the try costs one refused request and a move.
  Rejected for now: a fixed back-off (it needs a timestamp per limit in the stored usage).
- **A goal pause at a limit is the limit's** (refines ADR-0225). Decided where the pause is
  acted on (`goal_retry_observed`, `TranscriptFacts::fold`), by the limit the same turn recorded,
  not by wording: the notice "the request was rate limited" also follows refusals nobody else
  handles, and those still need a person.
- **Split by concern.** The person's first extra requirement (a manual refresh per subscription
  and for all, each card saying how old its reading is and which window is used up) is a page and
  probe feature of its own: it needs a probe request on the limited model to re-read "limit state
  now" (Claude's `anthropic-ratelimit-unified-7d_oi-*` headers) and a state computed when the page
  is shown. It is a chained task after this one, with its brief. Their second requirement (reset
  times known and used, tasks resume at the reset by themselves) is this change: UCL-003, UCL-004
  and UCL-006.
