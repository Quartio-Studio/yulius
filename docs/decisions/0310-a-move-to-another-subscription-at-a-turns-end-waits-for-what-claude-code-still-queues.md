# 0310. A move to another subscription at a turn's end waits for what Claude Code still queues

## Status

Accepted (2026-11-12). Specified in
[SPEC-479](../../.ai/specs/SPEC-479-a-subscription-switch-at-a-turn-s-end-lo.md); task #462,
issue #1194. Beside [ADR-0063](0063-background-work-and-goal-verdict-from-the-transcript.md)
(a move at a turn's end waits for background work) and
[ADR-0293](0293-claude-code-follows-latest-and-must-know-the-default-model.md) as amended by
SPEC-471 (the restart on a newer Claude Code waits for the queue). A default that can be
reversed: see Alternatives.

## Context

Claude Code keeps a prompt entered while a turn runs in a queue inside its process until the
Stop that ends the turn returned, or until the next tool result takes it into the running turn.
A resumed session does not get that queue back. Yulius types into running sessions (a person's
message through their Superagent or agent, a pull request's news, "Yulius is back") and tells
the sender "sent".

At every turn's end the pool decides whether the session stays, moves to another subscription
or is held (SPEC-034 rotation). A move ended the process with the Stop held. What waited in the
queue went with it: the resumed session never saw the message, and nobody was told. A turn held
for capacity that was later resumed on another subscription lost it the same way. Since
SPEC-471 Yulius counts the queue from the transcript's `queue-operation` records, but only the
restart on a newer Claude Code asked the count.

Claude Code also leaves some entries in its queue for good: notifications of work a sub-agent
started. Counted like prompts, they would keep a session from ever moving at a turn's end, and
it would run into its limit each time.

## Decision

- **A move the pool wants at a turn's end stays while a prompt waits.** The session is
  answered as on a subscription that stays (the gate blocks, a clean Stop ends the turn), the
  prompt is handed on or taken in by Claude Code itself, and a later turn's end moves the
  session. "A prompt waits" is: the queue holds an entry no turn's end left behind, or a
  hand-over is not over, or a prompt from the prompt box went on so recently, or while the
  turn's lines were read, that its record may be missing (SPEC-471's facts). The queue is asked
  after the turn's lines were read.
- **What a turn's end left in the queue keeps no move.** The record that closes a turn
  (`turn_duration`) is written after that end's `dequeue` records and not at a Stop a hook
  blocks. When that end handed nothing on, what the record finds waiting was left by Claude
  Code. The count keeps how many entries that is; an entry that leaves is taken to be one
  that was not left over, while there is one. An end that handed something on marks nothing:
  Claude Code hands a notification of its own, a slash command or a `!` line on alone, and a
  prompt behind it at the next turn's end (seen with 2.1.322 by the review, which found the
  first version of this rule losing that prompt).
- **A turn held for capacity is released in place, not moved, while a prompt waits.** The Stop
  is then asked again and the first rule decides.
- **The restart on a newer Claude Code keeps counting every entry** (SPEC-471, unchanged).

## Alternatives

- **Carry the waiting texts into the resumed session** (the issue's first idea, since "a
  subscription at its limit cannot wait"). It keeps to the threshold. It needs private prompt
  texts kept, a guess at which entry each unnamed `dequeue` took (a wrong one answers a prompt
  twice or never), an order with the gate's reasons and a goal typed again, and it turns a
  person's own prompt into text Yulius typed. The premise does not hold: on a subscription at
  its limit the handed-on turn is refused by Claude and continued past the limit on the next
  subscription, the path of every turn a limit cuts (SPEC-034, SPEC-072). Should one more turn
  over the threshold prove too dear, this is the alternative; the count and the left-over mark
  are what it would need too.
- **Tell the sender the message was not delivered.** Late (the sender was answered "sent"
  minutes before), and a person's own prompt has no sender.
- **Bound the wait by a number of turn ends or a timer.** A goal session whose Stops are
  blocked without a tool call keeps a real prompt waiting through any number of them; a person
  who writes at every turn would be cut off by a count.
- **Tell a notification from a prompt by its text.** A `dequeue` names no entry, so the text of
  what still waits is not known.
- **A leave takes a left-over entry first.** Every notification taken in during a turn would
  then make an old left-over entry count again, and such a session would stay at each turn's
  end.

## Consequences

- A session with a prompt waiting runs one more turn on a subscription over its threshold (or
  the rest of a turn the gate sent on). At its limit that turn is cut and redone after the
  move.
- A session that holds an entry Claude Code never takes out stays until a turn's end hands
  nothing on (usually the one at which the entry is first seen) and moves at the next.
- When a left-over entry leaves the queue alone while a prompt waits (a notification
  delivered to a sub-agent: once in this instance's 412 sessions with queue records), that
  prompt counts as left over, and a move at that turn's end loses it as before.
- Still lost, as with the restart: a prompt entered between the turn's end deciding to move
  and the task reading "Continuing", and one whose record Claude Code takes longer than two
  seconds to write. Not covered: a person's own switch or stop, a run without hooks, and what
  waits when a usage limit cuts a turn.
- The server's log says when a move waits ("the session is not moved to another subscription
  at this turn's end", "the held turn is not moved to another subscription").
