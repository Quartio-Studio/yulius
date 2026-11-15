# 0335. The Superagent's standing jobs are held by Yulius

## Status

Accepted. Specified in
[SPEC-487](../../.ai/specs/SPEC-487-the-superagent-can-do-everything-its-per.md) (CHK-001 to
CHK-005), task #469, slice 9b. Follows [0330](0330-the-superagent-does-what-its-person-may-do.md);
keeps [0150](0150-the-superagent-uses-agent-secrets-as-a-task-does.md)'s restart for
new tools (SPEC-487 slice 9a keeps the person's text across it).

## Context

Every restart of the Superagent (11 between 10 and 13 November: updates, usage limits, reboots)
ended its crons, watchers and background scripts: they live in its Claude Code process. Once
there was no monitoring for 15 hours while the person believed it was on (wall 8 of task #469).
The conversation itself survives a restart (it is resumed); the schedulers do not.

## Decision

- **Checks belong to Yulius.** `superagent_check_set {name, every_minutes, prompt}` (5 minutes to
  a day, a prompt of at most 2 KiB, at most 10 per Superagent), `superagent_checks` and
  `superagent_check_delete` keep a Superagent's standing jobs in `superagent-checks.json` beside
  the database, read back at a start before the server takes calls; audited without the prompt.
- **Typed when idle.** When a check is due and its Superagent's conversation is idle (no turn,
  no draft in its prompt box, no keystroke in the last minute, no restart pending), Yulius types
  its prompt into the session as one paste labelled `Yulius check "<name>"`, then Enter, and
  counts the next interval from then: a check never piles up behind a busy session.
- **Started when not running.** Every minute, the Superagent of a due check that is not running
  (after a reboot, or before its panel was ever opened) is started as opening its panel would,
  with its person's rights at that moment (an access lost or a disabled person starts nothing).
- **The instructions send standing jobs there**, never to a timer or loop of the Superagent's own.

## Alternatives

- **Automations.** They create tasks: a task per check reopens the board and costs a session
  per run, where the person asked their Superagent to watch.
- **Recreating the Superagent's crons after a restart.** Yulius cannot read or replay Claude
  Code's in-process schedulers, and a reboot would still lose them.
- **Delivering a check as a notification to the person.** The job is the Superagent's to do;
  the person asked it to watch, not to be reminded.

## Consequences

- "Keep monitoring the deploy" survives updates, usage limits and reboots.
- A due check can start a Superagent and use a subscription's capacity; at most 10 checks of at
  least 5 minutes bound it.
