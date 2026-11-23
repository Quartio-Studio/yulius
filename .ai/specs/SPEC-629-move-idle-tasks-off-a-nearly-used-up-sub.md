---
yulius-task: 493cd2d9-3bf8-4aa6-85d1-a41a5771227e
status: in-progress
---

# SPEC-629. Move idle tasks off a nearly used-up subscription before it runs out

## Goal

A subscription past a per-harness threshold (default 95 % of a window that binds the task)
gets no new task while another subscription of its provider has room. Idle tasks on it move
to the subscription with the most room before they need it, unless a person pinned them there.
Each move, and each task that cannot move because nothing has room, is told to the task's
person once and shown on the task.

## Context

The owner approved this on 22 Nov 2026 after a review of iTermX's `vm-account-switch`: every
10 minutes it checks the account new chats use. Past 95 % of its 5-hour or weekly window, new
chats go to the account with the most room, open chats on a nearly used-up account move there
once idle, and its inbox says what moved. The script was read as a reference only.

What Yulius does today (SPEC-001 ROT-003/004/007/010, SPEC-447, SPEC-611):

- `yulius_domain::rotation` ranks candidates below the 98 % `THRESHOLD`: known room first
  (Claude and Antigravity: 5 points below the threshold, Codex: 10 % left), then unknown usage,
  then tight accounts. Inside a tier Claude ranks by use-it-or-lose-it score, Codex by the most
  remaining quota (SPEC-611). A Claude subscription at 96 % with an earlier weekly reset can
  therefore beat one at 94 %; nothing keeps a nearly used-up subscription out of placement
  until 98 %.
- A task moves proactively only at a turn boundary (the Stop hook, or a hookless run's turn
  end flagged by `record_account_usage`) and only past 98 %. A task that is already idle when
  its subscription crosses the threshold waits, then hits the limit or pauses at its next turn.
  This week several Codex subscriptions sat at 98–100 % (Ultra 500 at 99 %).
- `App::move_task` (SPEC-447) moves a running idle task in the same session with nothing typed
  and keeps its Waiting or Done activity (SPEC-611 CQS-001). A person's "Run on…" and
  "Continue on…" are honoured below 100 %, but nothing remembers the choice: a parked task
  resumes wherever the pool places it.
- An idle session is restarted for a newer Claude Code only when nothing would be lost
  (`agent_cli::kept_by`, SPEC-461 CCM-013, SPEC-471): a person typed in the last minute, a draft,
  a person's pause, an instance update holding runs, a refused switch moments ago, background
  work, Claude Code's queue, a prompt whose record may be unread.
- The task's info lists its subscription switches with a reason; a terminal notice names
  accounts only for refused or removed subscriptions (MOV-006). Notices for people have keys
  and expiries since SPEC-624 (task #608, merged into dev while this was reviewed): a newer
  notice with the same project, recipient and key replaces the older one, and they are stored,
  so a page opened later and the person's Telegram (by their choices) get them too.
- Parked tasks (SPEC-512 PRK) hold no session and no subscription; their resume is a placement.

## Requirements

- **RBL-001** Each harness has a rebalance threshold in percent of a window that binds its
  task's model (5-hour, weekly, and the model's own weekly window such as Fable's or Opus's):
  `YULIUS_REBALANCE_PERCENT_CLAUDE_CODE`, `YULIUS_REBALANCE_PERCENT_CODEX` and
  `YULIUS_REBALANCE_PERCENT_ANTIGRAVITY`, each 50–98, default 95. A subscription is past it
  when a binding window is at or above it or a limit is in effect. Unknown usage is not past it.
  At 98 (the pool's own threshold) the harness's rebalance is off: placement and moves are as
  before.
- **RBL-002** Placement (a new task, Continue, a resume, a queued or paused start, and the
  target of any automatic move) ranks every candidate past the rebalance threshold after every
  candidate below it; within each group the existing ranking (tier, Codex remaining quota,
  score, ties) applies. A subscription past it is still used when no other has room, up to the
  98 % threshold as before. A person's explicit choice is not affected.
- **RBL-003** Every 2 minutes (the first pass 3 minutes after start), Yulius looks at each
  running task whose live run is on a subscription past its harness's threshold for the model
  that run was launched with. It moves the task, in the same session and with nothing typed,
  to the best subscription below the threshold (RBL-002), when the task is idle: its status is
  `running`, its activity `waiting` or `done` (the turn ended, also while a Needs you request
  waits for a person), and nothing would be lost (the `kept_by` facts of SPEC-461: no keystroke
  in the last minute, no draft, no person's pause, no instance update holding runs, no switch
  refused in the cooldown, no background work or one that cannot be told, nothing in Claude
  Code's queue, no prompt whose record may be unread, and no goal of the person's own open in
  a prompt-mode session, no usage limit it waits at, no first message still to arrive). Never a
  task mid-turn (`working`, or `needs_attention` such as a permission prompt), switching,
  starting, paused or parked. The move claims the idle activity, update time and run it read,
  for every harness: a prompt that arrives meanwhile wins and the task stays. Moves run one
  after the other; a subscription a move to failed takes no further move in that pass.
  Only a session that reopens at its prompt with nothing typed is moved: a prompt-mode session,
  or a Codex one in either mode, with no unfinished workflow or cut-off reviewer. Any other idle
  session (a goal session of Claude Code or Antigravity, whose goal a restart types again) is
  parked instead when parking is on and its rules allow it (SPEC-512 PRK-002, with a quiet time
  of one minute or the instance's grace period if shorter, asked from the local facts before a
  park verifies the task's pull requests on GitHub), so its resume is placed off the
  subscription (RBL-004); otherwise it stays, with the reason logged.
- **RBL-004** A parked task holds no subscription. Its resume is a placement (RBL-002), so it
  resumes off a subscription past the threshold unless it is pinned (RBL-005).
- **RBL-005** A person's choice of a subscription pins the task to it: "Run on…" (`switch` with
  `account_id`), "Continue on…" (`continue` with `account_id`) and a fork started on a named
  subscription (`fork` with `account_id`, SPEC-628) store the chosen subscription
  as the task's pin. The rebalance never moves a task whose live run is on its pin, and a
  parked task resumes on its pin while the pin can take it (the person's-choice rule, below
  100 %), else where the pool places it. A pin ends once the task runs on another subscription
  (the pool moved it at a usage limit or at 98 % at a turn boundary, the pinned subscription was
  removed or refused, the person chose another, or a resume was placed elsewhere): every launch
  on another subscription ends it, and so do a Continue or "Run on…" without a subscription and
  the removal of its subscription.
- **RBL-006** Each move is told once, once the task runs on the target: the task's terminal says it in one line ("↻ Claude
  a-first is at 96% of its 5-hour window: this idle session continues on Claude b-second."),
  the task's info lists the switch with the reason `rebalance: Claude a-first is at 96% of its
  5-hour window`, and the task's person (its owner) gets one notice (SPEC-624): "#12 Checkout
  moved to Claude b-second", "Claude a-first is at 96% of its 5-hour window.", keyed
  `rebalance:<task id>` in the task's project, category `account`, tone info, linked to the
  task, expiring after a day; its next move, park or wait replaces it.
  The switch's own lines ("↻ Yulius continues this session on another Claude subscription…")
  stay as for any move. A session parked instead (RBL-003) says so in one line ("⏸ Claude
  a-first is at 96% of its 5-hour window: this idle session is parked and continues on a
  subscription with room when it is written to next.") and one notice ("#12 Checkout parked
  off Claude a-first"), and its
  parked screen says why ("Yulius parked this session because Claude a-first is at 96% of its
  5-hour window: it continues on a subscription with room."). Terminal lines reach only the
  pages open at the time; the notice, a move's switch in the task's info and a park's parked
  screen last.
- **RBL-007** An idle, unpinned task on a subscription past the threshold with no subscription
  below it says so once per episode: one terminal line ("⏸ Claude a-first is at 97% of its
  week and no other Claude subscription has room: this task stays on it and moves when one
  has.") and one notice ("#12 Checkout stays on Claude a-first", tone warn, the same key). The
  episode ends when the task moves or its subscription is below the threshold again, and then
  its notice is removed (a move or a park replaces it); a Yulius restart may tell it once more.
- **RBL-008** Every rebalance move is a recorded switch decision with its inputs (ROT-011), and
  each pass logs what it kept and why.

## Plan

- [x] Specify proactive rebalancing and record the decision
- [x] Place new tasks off subscriptions past a per-harness threshold
- [x] Decide idle moves in the domain: move, stay, or nothing has room
- [x] Pin a task to the subscription a person chose
- [x] Move idle tasks off a subscription past the threshold every two minutes
- [x] Tell each move and each task that cannot move, once
- [x] Document the rebalance in product, operations, architecture and changes
- [x] Answer the first review: goal sessions parked, the idle claim, untested guards
- [x] Pass the independent review of the exact head
- [x] Remove a stale "no room" notice and pin a fork to its chosen subscription
- [x] Pass the independent review of the follow-up
- [x] Repair the red staging gate: the agent shim passes `yulius run`
- [ ] Pass the independent review of the staging repair

## Acceptance criteria

- **AC-1** With Claude subscriptions at 96 % (weekly reset in 1 hour) and 94 % (reset in 100
  hours), and with Codex subscriptions at 96 % and 94 %, a new task goes to the 94 % one. With
  every candidate past 95 % and below 98 %, a new task still starts on the best of them.
- **AC-2** A task idle on a subscription that reads 96 % moves at the next pass to the one at
  30 %, in the same session, with nothing typed; its terminal says the move once, its info
  lists the switch with the reason, and its person gets one notice keyed `rebalance:<task id>`.
- **AC-3** A task mid-turn (activity `working`) on a subscription at 96 % stays through a pass;
  its turn's end at 96 % is not a move by itself, the next pass after it moves it.
- **AC-4** A task a person moved with "Run on…" to a subscription that then reads 96 % stays
  there through passes. A parked task pinned to a subscription below 100 % resumes on it.
- **AC-5** With every other subscription at or above 95 %, an idle task on one at 97 % stays,
  its terminal and its person are told once, and later passes tell nothing more until the
  episode ends.
- **AC-6** A threshold outside 50–98 is refused at start, naming the argument; each reads its
  own variable and is 95 by default.

## Test plan

Domain unit tests in `rotation.rs` and the new `rebalance` module for placement past the
threshold (Claude score and Codex quota orders, all past it, the model's own weekly window),
the idle decision (move, stay below the threshold or on unknown usage, nothing has room), the
kept facts and the window names. Integration tests in `crates/yulius/tests/rebalance.rs` (and a
Codex one in `codex.rs`) with real PTY hosts and the fake agent, as SPEC-447's: placement, an
idle move, no move mid-turn, a pinned task stays, nothing has room told once, a pinned parked
task resumes on its pin (or where the pool places it when the pin is full), a draft or a
person's pause keeps a task, a prompt during the switch wins, an idle goal session is parked
and never restarted, pins end, and an idle Codex goal task moves and stays Done. The fake agent
runs no hooks, so the tests record a turn's end in the store as the Stop hook would. The store
test pins the migration. Each new test is shown to fail without its change (Evidence). Run the
touched binaries in full, `scripts/dev quick`, and the e2e notification spec if the center
changes.

## Decisions

- One threshold per harness from the server's configuration, not a setting per subscription:
  the owner asked for one rule per harness, and the Settings page grows nothing to manage
  (ROT-009: no priority, order or switch per subscription).
- The rebalance threshold only orders placement and picks idle tasks; the 98 % threshold of
  ROT-003/010 still decides pausing and the turn-boundary move. Moving working tasks at 95 %
  would cut nothing but would restart sessions mid-goal at every turn; idle ones cost nothing.
- A periodic pass, not a reaction to usage readings: an idle Claude subscription gets no
  readings (its sessions report its usage), so only a clock finds its idle tasks. Two minutes
  matches the background usage round; iTermX's 10 minutes is slower than Yulius's readings.
- Move through `App::switch_from` with nothing typed and SPEC-611's idle claim
  (`SwitchCondition::Idle`, now for every harness), and ask the facts of `agent_cli::kept_by`
  (`kept_idle`) for "nothing would be lost", so a rebalance is as safe as a person's move and
  as a restart for a newer Claude Code; a session whose restart would start a turn is parked.
- The pin is a column of its own (`tasks.pinned_account_id`) behind two store calls, not a
  field of `TaskRecord`; a pin that no longer names the run's subscription is cleared.
- Told in the terminal, the task's info and one notice per task (SPEC-624), key
  `rebalance:<task id>` so the latest of its moves, parks or waits stands, in category
  `account`: it reaches Telegram only for a person who chose "Subscription problems" there (off
  by default, offered to instance administrators; its description says that moves come too),
  since a move needs nobody to act.
- [Decision 0461](../../docs/decisions/0461-move-idle-tasks-off-a-subscription-past-a-per-harness-threshold.md)

## Evidence

Each test below was shown to fail with the line it guards undone, then restored:

- Placement (RBL-002): `rotation::tests::placement_keeps_off_a_subscription_past_the_rebalance_threshold`
  failed before the ranking (left `early`, right `roomier`). `rebalance::a_new_task_keeps_off_a_subscription_past_the_threshold`
  fails with the past-threshold sort key removed (left `[a-first]`, right `[b-second]`).
- Idle move, mid-turn, pin (RBL-003, RBL-005): with the idle-activity guard widened to any activity,
  `rebalance::a_task_mid_turn_stays_until_its_turn_ends` fails (Working: moved 1). With the pin check
  disabled, `rebalance::a_pinned_task_stays_on_its_subscription` fails (moved 1, pinned 0). With
  `pinned_start` replaced by `None`, `rebalance::a_pinned_parked_task_resumes_on_its_pin` fails
  (left `a-first`). With the launch's pin clear disabled, `rebalance::the_pools_move_ends_a_pin` and
  `rebalance::a_parked_task_whose_pin_is_full_resumes_where_the_pool_places_it` fail. With the
  pass's pin clear disabled, `rebalance::the_pass_ends_a_pin_the_task_left` fails.
- Nothing lost (RBL-003): with `kept_idle` ignored, `rebalance::a_draft_or_a_persons_pause_keeps_an_idle_task`
  fails. With `SwitchCondition::Idle` replaced by `Any`,
  `rebalance::a_turn_that_starts_while_the_move_is_prepared_keeps_its_run` fails (it holds
  `race::Point::SwitchPrepared` and starts a turn).
- Goal sessions (RBL-003, RBL-004): with `restart_starts_a_turn` ignored,
  `rebalance::an_idle_goal_session_is_parked_not_restarted` fails (moved 1). With the park reason
  dropped, it fails on the parked screen. With the local `parkable_now` check before the park
  removed, `rebalance::a_goal_session_parking_keeps_costs_no_github_reads` fails (a GitHub App is
  configured; the park lists pull requests).
- Off switch (RBL-001): with the pass's 98 check moved to 99, `rebalance::at_98_the_rebalance_is_off`
  fails. `serve::tests::each_harness_has_a_rebalance_threshold_of_50_to_98` checks the variables,
  the default and the 50–98 range.
- Told once (RBL-006, RBL-007): with the episode check removed, `rebalance::nothing_has_room_is_told_once`
  fails (three lines). With the notice not posted, `rebalance::an_idle_task_leaves_a_subscription_past_the_threshold`
  and `rebalance::nothing_has_room_is_told_once` fail; the first also reads the notice back from
  `my_notices` (key `rebalance:<task id>`, with its link to the task).
- Codex: `codex::a_rebalance_moves_a_finished_codex_goal_task_and_keeps_it_done` moves an idle Codex
  goal task in the same conversation with `prompt: None` and keeps Done.
- Store: `a_pin_names_the_chosen_subscription_until_it_is_removed`; the migration list pins schema
  92 to `0090_task_account_pin.sql`, after dev's 0089, 0091, 0088 and 0092.

Runs at the final code (merged with dev at 6611b4d54): `scripts/cargo nextest run -p yulius-domain
-p yulius-store -p yulius-app -p yulius-contracts -p yulius` ran 4964 tests: 4963 passed and
`parking::a_staging_outcome_reaches_its_task_once` failed once under that load. It was on dev before
this change and touches no rebalance code; alone it passed 3 of 3 and 30 of 30 with
`--stress-count 30`; filed as #1712. `scripts/cargo clippy --locked --workspace --all-targets
--all-features -- -D warnings` and `cargo doc --workspace -D warnings` are clean; `scripts/dev
quick` passed. Yulius CI run af8eaa6a failed `docs` at `ba45e49` (private intra-doc links), fixed
in `84fcae6e5`; the runs at `84fcae6e5` and `52162c900` passed. `yulius security-scan` printed
"Security scan disabled" (Vidoc is off for this project; nothing was scanned, not a pass).

Reviews: round 1 (Fable 5.2) at `ba45e49` asked for changes (1 blocker, 3 major, 7 minor, 2 nit),
answered in `84fcae6e5`, `9df7cc5b5` and the tests after it. Round 2 at `ac6020ef1` asked for changes
in the park path (2 major, 4 minor, 3 nit), answered in the commits after it. Round 3 approved
`52162c900` (3 minor, 1 nit left). After it, dev brought SPEC-624's keyed notices: the live
`task.rebalanced` event became one stored notice per task (`rebalance:<task id>`), and a switch
whose old run did not end in time marks its target failed. A follow-up after the merge (#1707) answered round 4's minors: a "stays on" notice is
removed once its subscription has room again (`rebalance::nothing_has_room_is_told_once` fails
without it), a fork started on a named subscription is pinned there
(`forks::a_codex_fork_waits_with_its_subscription_then_resumes_its_own_session` fails without
it), and the Telegram choice says that moves come under it. Round 5 approved `5a4893dea` (one nit
left: after a Yulius restart a "stays on" notice can stand until its one-day expiry). Staging request 8c86a433 for the merged revision 7dc2641 failed its full gate (run 72091baf):
`tests::the_agent_wrapper_passes_every_session_command` (crates/yulius/src/main.rs) found the
session command `yulius run`, which #1715 (SPEC-626 BRK-007) added, missing from the agent's
`yulius` shim. Yulius asked this task to fix it forward: `agent_yulius_shim` passes `run`, and the
test that failed on dev passes.

Not tested here: real Claude Code, Codex or Antigravity subscriptions and their usage readings
(synthetic usage and the fake agent stand in); the fake agent runs no hooks, so the tests record a
turn's end in the store as the Stop hook would, and the goal-session test records goal mode on a
settled prompt-mode session; an Antigravity session, and the workflow and cut-off-reviewer
branches of `restart_starts_a_turn`, have no test of their own (they reuse the `rest` rule of
`switch_from`); the one-minute quiet time is tested only for "not before" (the tests run on the
system clock).

