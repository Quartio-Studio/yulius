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
  therefore beat one at 94 %, and a Codex one at 96 % beats nothing in its tier.
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
  accounts only for refused or removed subscriptions (MOV-006). The notification center
  (`notifications.js`) keeps keyed entries per browser. Server-side notice keys and expiry are
  task #608 (SPEC-624), not merged yet: this change keys its own entries in the center.
- Parked tasks (SPEC-512 PRK) hold no session and no subscription; their resume is a placement.

## Requirements

- **RBL-001** Each harness has a rebalance threshold in percent of a window that binds its
  task's model (5-hour, weekly, and the model's own weekly window such as Fable's or Opus's):
  `YULIUS_REBALANCE_PERCENT_CLAUDE_CODE`, `YULIUS_REBALANCE_PERCENT_CODEX` and
  `YULIUS_REBALANCE_PERCENT_ANTIGRAVITY`, each 50–98, default 95. A subscription is past it
  when a binding window is at or above it or a limit is in effect. Unknown usage is not past it.
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
  a prompt-mode session). Never a task mid-turn (`working`, or `needs_attention` such as a
  permission prompt), switching, starting, paused or parked. Moves run one after the other.
- **RBL-004** A parked task holds no subscription. Its resume is a placement (RBL-002), so it
  resumes off a subscription past the threshold unless it is pinned (RBL-005).
- **RBL-005** A person's choice of a subscription pins the task to it: "Run on…" (`switch` with
  `account_id`) and "Continue on…" (`continue` with `account_id`) store the chosen subscription
  as the task's pin. The rebalance never moves a task whose live run is on its pin, and a
  parked task resumes on its pin while the pin can take it (the person's-choice rule, below
  100 %), else where the pool places it. A pin ends once the task runs on another subscription
  (the pool moved it at a usage limit or at 98 % at a turn boundary, the pinned subscription was
  removed or refused, or the person chose another), and when its subscription is removed.
- **RBL-006** Each move is told once: the task's terminal says it in one line ("↻ Claude
  a-first is at 96 % of its 5-hour window: this idle session continues on Claude b-second."),
  the task's info lists the switch with that reason, and the task's person gets one
  notification center entry (event `task.rebalanced`, keyed per move).
- **RBL-007** An idle, unpinned task on a subscription past the threshold with no subscription
  below it says so once per episode: one terminal line ("⏸ Claude a-first is at 97 % of its
  week and no other Claude subscription has room: this task stays on it and moves when one
  has.") and one notification center entry keyed per task, subscription and episode. The
  episode ends when the task moves or its subscription is below the threshold again; a Yulius
  restart may tell it once more.
- **RBL-008** Every rebalance move is a recorded switch decision with its inputs (ROT-011), and
  each pass logs what it kept and why.

## Plan

- [x] Specify proactive rebalancing and record the decision
- [x] Place new tasks off subscriptions past a per-harness threshold
- [x] Decide idle moves in the domain: move, stay, or nothing has room
- [ ] Pin a task to the subscription a person chose
- [ ] Move idle tasks off a subscription past the threshold every two minutes
- [ ] Tell each move and each task that cannot move, once
- [ ] Document the rebalance in product, operations, architecture and changes
- [ ] Pass the independent review of the exact head

## Acceptance criteria

- **AC-1** With Claude subscriptions at 96 % (weekly reset in 1 hour) and 94 % (reset in 100
  hours), and with Codex subscriptions at 96 % and 94 %, a new task goes to the 94 % one. With
  every candidate past 95 % and below 98 %, a new task still starts on the best of them.
- **AC-2** A task idle on a subscription that reads 96 % moves at the next pass to the one at
  30 %, in the same session, with nothing typed; its terminal says the move once, its info
  lists the switch with the reason, its person's event stream carries one `task.rebalanced`.
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
kept facts and the window names. Integration tests in `crates/yulius/tests/subscription_moves.rs`
with real PTY hosts and the fake agent, as SPEC-447's: an idle move, no move mid-turn, a pinned
task stays, nothing has room told once, and a pinned parked task resumes on its pin. The store
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
- Reuse `App::move_task` for the move (SPEC-447, SPEC-611's idle preservation and conflict
  guard) and the facts of `agent_cli::kept_by` for "nothing would be lost", so a rebalance is as
  safe as a person's move and as a restart for a newer Claude Code.
- The pin is a column of its own (`tasks.pinned_account_id`) behind two store calls, not a
  field of `TaskRecord`; a pin that no longer names the run's subscription is cleared.
- Told in the terminal, the task's info and the notification center; not by Telegram, whose
  messages are for what needs a person (SPEC-498). Keys are the center's until SPEC-624 gives
  server notices keys.
- [Decision 0461](../../docs/decisions/0461-move-idle-tasks-off-a-subscription-past-a-per-harness-threshold.md)
