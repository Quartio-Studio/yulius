# 0293. Claude Code follows `latest`, and a release must know the default model before it is used

## Status

Accepted (2026-11-11). Specified in
[SPEC-461](../../.ai/specs/SPEC-461-claude-code-in-sandboxes-always-knows-yu.md); task #443.
Amends [ADR-0266](0266-yulius-keeps-claude-code-current-in-its-data-directory.md): its default
channel (`stable`), what a release must be before it becomes current, and "a running process keeps
its binary until its next start". The rules of the move were refined on 2026-11-11 by
[SPEC-468](../../.ai/specs/SPEC-468-claude-code-moves-the-last-review-s-left.md) (task #452):
what else keeps a session where it is, and that an idle restart is no news; and on 2026-11-12 by
[SPEC-471](../../.ai/specs/SPEC-471-claude-code-moves-a-prompt-that-waits-in.md) (task #454,
issue #1204): what waits in Claude Code's queue keeps the session.

## Context

Since 2026-11-10 tasks default to `claude-fable-5-2` (ADR-0289). On 2026-11-11 production ran
Claude Code 2.1.311, npm's `stable`, while `latest` was 2.1.322. The 2.1.311 binary does not hold
`claude-fable-5-2`: the API still answered with it, because Yulius passes the id, but the
session's own model picker, its `fable` alias, and whatever Claude Code chooses by itself
(subagents) meant Fable 5.1. Nothing said so: the doctor line was always `ok`, and the channel was
a start-up argument a person had to edit in the unit. A Claude Code binary names the models it
knows as plain text (2.1.322 holds `claude-fable-5-2` 26 times, 2.1.311 not once).

## Decision

- **`latest` is the default channel.** `YULIUS_CLAUDE_UPDATES` keeps `stable` and `off`.
- **A release is read for the models the composer offers before it becomes current**, in the pass
  that already checks its version, without running it. One that does not know the default model
  is refused and reported. One that lacks another offered model is taken and the gap reported.
- **`stable` falls forward to `latest`** when neither the version in use nor stable's release
  knows the default model.
- **The version in use is checked the same way** at start, after each check and when the channel
  changes. `yulius doctor` fails on a Claude Code that does not know the default model, and
  Settings, Instance shows a "Claude Code" card with the channel, the versions, the last check,
  "Check now" and the warnings. The state is also written to `<data>/claude-code/status.json` for
  the doctor.
- **The channel is an instance setting** (`instance.claude_channel`), changed on that card and
  applied without a restart; it wins over the unit's `YULIUS_CLAUDE_UPDATES`.
- **A running task moves at a turn boundary.** Each session records the Claude Code version its
  transcript names. When a turn ends and the session runs an older one than runs get now, Yulius
  restarts it on the newer one in the same Claude session, on the same subscription, through the
  subscription switch (reason `claude_code`): never while background work runs (a `yulius wait`
  included), a person typed in the last minute or left a draft (typed since the session's last
  prompt), a message Yulius typed into it is not submitted yet, or a goal its person set in a
  prompt-mode session is open (Yulius could not follow it across the restart); once per version,
  one restart of a task at a time. Nor while something waits in Claude Code's queue: a prompt
  entered while a turn runs (a person's, or a message Yulius typed) and Claude Code's own
  notifications are kept in the process's memory alone, and a resumed session does not get them
  back. Yulius reads the queue from the `queue-operation` records Claude Code writes into the
  transcript (`enqueue`, `dequeue`, `remove`, `popAll`, `popOne`), counted per run and not
  known, which counts as holding, until a run reattached after a restart of the server was
  read again; an entry that was handed on counts until its turn shows (its prompt record, or
  the model's first line written after the hand-over); and for two seconds after a prompt
  from the prompt box went on, or when one goes on while the turn's end looks, the session
  stays as well, since that record is written a moment after the prompt's hook. Such a
  session moves at a later turn's end, and the server's log names what kept it (SPEC-471). An idle session
  that is restarted reads as it did before, so nobody is notified again. A version a session could
  not continue on gets no further moves: the restart could not start, or the restarted session
  ended by itself or stayed silent for two minutes before it started. A restart refused before it
  began has its own ten-minute wait and is no switch decision. A goal session moves only while its
  goal is set and unmet.

## Alternatives

- **Refusing a release that lacks any offered model.** It stops every update once a retired
  model's id leaves the binary while the composer still lists it: Claude Code falls behind for a
  list entry. The default model is the gate; the rest is reported.
- **Running `claude --version` or asking the binary for its models.** It would run a fresh
  download as the service user, with the database and the vault key in reach (ADR-0266).
- **Keeping `stable` and pinning a minimum version per default model.** A table to maintain by
  hand that is wrong the day a model ships; the binary itself says what it knows.
- **Storing the channel in a file under `claude-code/`.** An instance export leaves that directory
  out, so the setting would be lost in a move.
- **Restarting idle sessions when a release is installed.** A resumed goal session gets its goal
  typed again, which starts work in a task that was waiting for its person.
- **A new restart path for the move.** The switch already checks that the new run can start
  before it ends the old one, yields to a stop or close, and refuses to end background work.
- **Knowing Claude Code's queue from Yulius's own hooks** (SPEC-471): a mark at a prompt hook
  that arrives while a turn is open, cleared by the queued prompt's record. It needs Yulius's
  reading of "a turn is open", which is late after an interrupt; a prompt taken into the
  running turn at a tool result, or back into the prompt box, writes a different record; and
  it does not know Claude Code's notifications. A mark that is never cleared keeps a session
  for good. The transcript's `queue-operation` records are Claude Code's own count.
- **Carrying the queue over a restart** by typing the `enqueue` records' texts into the
  resumed session (SPEC-471): the move can wait, so it does; a subscription switch that cannot
  wait is issue #1194.

## Consequences

- Claude Code leaves some of its own notifications in its queue for good (those of work a
  sub-agent started; about 2 in 100 of this instance's sessions held one): such a session
  never moves at a turn's end and gets the newer version at its next start. So does one with
  an operation Yulius does not know, or one cleared in its terminal with entries waiting: a
  count that is too high errs toward staying (SPEC-471).
- Sandboxes get each Claude Code release within an hour of its publication, and running tasks at
  the end of their next turn. A release that breaks sessions reaches them sooner than under
  `stable`. Choosing Stable then keeps later releases away but does not move `current` back; Off
  is the click that changes what runs get (the host's install), and a session that cannot
  continue on a version stops further moves to it.
- The check reads model ids as plain text in the binary. A Claude Code that stopped naming them
  so would be refused release after release, loudly; the host's install, which runs get when it
  is newer, is the way through until the check is adapted.
- A default model newer than every Claude Code release is refused loudly instead of being run
  half-known; the default should not move before a release knows it. "No release knows it yet"
  is said only from the server's read of the registry in the last two hours, on a channel that
  reads it: a remembered `latest` is not the newest release.
- The model check reads the downloaded binary once (about 340 MB) and the version in use once per
  change; nothing is executed.
- A task whose session was restarted for a new version shows one more session in its info, with
  the version each ran.
