# 0346. A cut-off reviewer is resumed after its session moves

## Status

Accepted (2026-11-16). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (RLM-001 to
RLM-004, fix 7); task #472. Extends [ADR-0326](0326-a-workflow-agents-usage-limit-is-the-sessions.md)
from a dynamic workflow's agents to the session's own subagents, and
[ADR-0341](0341-yulius-picks-the-model-of-each-review-round.md)'s move for a Fable reviewer
(REV-007) to a reviewer cut off in the middle of its round.

## Context

Of the first reviews since 11 Nov 12:00, 14 of 86 on Fable at high effort and 35 of 62 at max
ended with no report: the subscription's limit cut the reviewer subagent off. 24 review rounds
on 12-13 Nov were "continue, you were cut off", and 9 of the 38 re-reviews that asked for changes
were such resumptions. Since fix 1 an Opus session runs a Fable reviewer, so a limit of Fable
refuses the reviewer while the session itself goes on: Claude Code writes the refusal to the
reviewer's transcript only, sends no `SubagentStop`, and tells the main thread with a failed task
notification. `SendMessage` to the same agent id resumes it with all it had read, on the same
model, also after the process was restarted with `--resume` (measured with Claude Code 2.1.325).

## Decision

- **A subagent's limit is the session's news** (RLM-001): the harness reports the refusal in a
  subagent's transcript with what it binds; a limit of "the model that ran" names the family of
  the model that answered that subagent. The subscription's reading keeps it for that family. A
  limit of the session's own model (or the subscription's) moves or pauses the session as any
  limit does.
- **A cut-off reviewer moves the session** (RLM-002): when the refused subagent is an open review
  round's reviewer and the limit binds its model, not the session's, the session moves to a
  subscription that can run both models, at a turn's end: an idle session's thread is woken by
  the reviewer's failed task notification, and that turn's Stop hook moves it, with the gate's
  answer, after reading the turn's lines (a move waits while background work runs or a prompt is
  queued). Nothing moves a session outside a turn's end: a round exists only with hooks, so its
  Stop hook always comes. The new process's first prompt tells it to resume that reviewer with
  `SendMessage` to its id and not to start a new one; a relaunch past a limit of the session's
  own model says the same. With no such subscription the session is told to start the review
  again, which then runs on Opus 5.5 (REV-007).
- **A round does not start about to run out** (RLM-003): when the session's subscription has
  under 5 points of room (`Tier::Tight`) in a window binding the reviewer's model and another
  has known room, the reviewer's start is refused and the session moves first, as for Fable. A
  move for room gives way to background work or a queued prompt: the round then starts where it
  is.
- **One rule** (RLM-004), held by a test everywhere reviews are described: "A usage limit can cut
  a reviewer off before its report: resume that same reviewer with SendMessage to its agent id,
  never start a new one for the same round. A reviewer runs at high effort, never max."

## Alternatives

- **Leave it to the agent.** Authors who knew resumed; others started a fresh reviewer and paid
  for the reading again.
- **Move at every cut-off at once, mid-turn.** It would end the agent's work in progress; the
  turn's end is close, and the reviewer waits.
- **Refuse a tight round while work runs.** A session that always has a `yulius wait` running
  would never be reviewed; starting where it is risks only a cut-off, which RLM-002 answers.

## Consequences

- A cut-off reviewer costs one move and one message, not a new round's reading.
- The move reasons `review_resume` and `review_room` show in the task's switches.
- A week later: the analytics read's share of first rounds with no report (baseline 14 of 86 at
  high effort) and rounds per pull request (fix 1's read); `SELECT reason, count(*) FROM switch_decisions
  WHERE reason IN ('review_resume', 'review_room') GROUP BY reason` counts the moves.
