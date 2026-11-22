# 0461. Move idle tasks off a subscription past a per-harness threshold

## Status

Accepted for [SPEC-629](../../.ai/specs/SPEC-629-move-idle-tasks-off-a-nearly-used-up-sub.md).
Amends SPEC-001 ROT-003, ROT-004 and ROT-007, [ADR-0288](0288-a-person-moves-a-task-between-subscriptions-and-removing-one-moves-its-tasks.md)
and [ADR-0449](0449-use-codex-quota-headroom-and-preserve-idle-moves.md).

## Context

The pool moves a task off its subscription only at a turn boundary past 98 %, or at a usage
limit. A task already idle when its subscription crosses that line waits, then hits the limit or
pauses at its next turn. Placement can also choose a subscription at 96 % over one at 94 %: the
Claude score prefers an earlier weekly reset, and a Codex subscription at 96 % is merely "tight".
In the week of 22 November 2026 several Codex subscriptions sat at 98–100 %. iTermX's
`vm-account-switch` solves the same problem with a soft line at 95 %: new chats avoid an account
past it, and open chats leave it once idle.

## Decision

Each harness gets a rebalance threshold (`YULIUS_REBALANCE_PERCENT_<HARNESS>`, 50–98, default
95) in percent of the windows that bind the task's model. Placement ranks every candidate past it
after every candidate below it, keeping the existing order inside each group; the 98 % threshold
still decides pausing and turn-boundary moves.

A pass every 2 minutes moves each idle, unpinned task whose live run sits on a subscription past
the threshold to the best subscription below it, through the person's move (`App::move_task`)
and only when the restart for a newer Claude Code would also lose nothing (`agent_cli::kept_by`).
The pass moves only a session that reopens at its prompt with nothing typed, claiming the idle
state and run it read; a goal session of Claude Code or Antigravity, whose goal a restart would
type again, is parked instead under parking's rules, so its resume is placed elsewhere. With no
subscription below the threshold the task stays and says so once per episode.

A person's "Run on…" or "Continue on…" pins the task to the chosen subscription
(`tasks.pinned_account_id`). The pass never moves a task running on its pin, and a parked task
resumes on its pin while the pin can take it. The pin ends once the task runs elsewhere.

Each move is told once: one terminal line, the switch with its reason in the task's info, and a
keyed notification center entry for the task's person (`task.rebalanced`).

## Alternatives

- Lower the 98 % threshold to 95 %: would also restart working sessions at every turn boundary
  and pause the pool three points earlier, wasting capacity.
- React to usage readings instead of a clock: an idle Claude subscription gets no readings
  (only running sessions report usage), so its idle tasks would never be found.
- A threshold per subscription on the Subscriptions page: more to manage, against ROT-009.
- Pin every automatic placement as well, or add a pin toggle: a person's explicit choice is the
  only signal there is; an extra control would need a UI nobody asked for.
- Telegram messages for every move: those messages are for what needs a person.

## Consequences

Subscriptions are spent more evenly and idle tasks are rarely caught at a limit. A rebalance
restarts the idle session once (as a person's move does), which is visible as a short
"Continuing". A pinned task can still run into its limit and is then moved by the pool as
before, which ends the pin. The "nothing has room" memory is in the process: a restart of Yulius
may tell an episode once more. Notices are keyed in the browser's notification center until
server-side notice keys (SPEC-624) exist.
