# 0311. A move is finished only while its old instance is held, and every run looks

## Status

Accepted. Specified in
[SPEC-481](../../.ai/specs/SPEC-481-instance-migrate-a-run-that-could-not-fr.md); task #461,
issue #1179. Refines [0297](0297-one-root-command-moves-an-instance-and-drives-the-new-host-over-a-one-time-key.md)
("Idempotent steps, little state"; "The old unit is disabled only once the new instance is
healthy").

## Context

- Once the new instance of a move reports the build it has taken over (its first start queued
  the tasks), so nothing after that is a rollback (SPEC-460, SPEC-467 LFT-002). The old instance
  must then never run beside it (SPEC-460 MIG-004): frozen and disabled.
- The command could fail at exactly that: `systemctl disable` fails, a unit is thawed from
  outside between the start and the look, a freeze of a unit that runs beside the new instance
  fails. It said so in one `FAIL` line, recorded the move (`Cutover::Moved`), finished it
  (`finished`), and the next run answered "finished" with exit 0 from the record alone. The
  line lived in a checklist the next run overwrote (issue #1179).
- A move's state file is small on purpose (ADR-0297): the move's identity and the cutover's
  progress, so a rerun cannot disagree with the hosts. "The old instance is held" is a fact of
  the old host that changes without the command: a person thaws, starts, stops or enables a
  unit; `systemctl stop` of a frozen unit thaws it and leaves its agents on systemd 255.
- After the finish the one-time key is gone: a run on the old host can no longer ask the new
  instance anything.

## Decision

- **The state file keeps two facts and no third.** `Moved`: the new instance has taken over.
  `finished`: the finish was done (the key is gone on both hosts). Neither says that the old
  instance is held.
- **Every run of a moved move asks systemd, with one look** (`Move::old_hold`): no unit of the
  old instance runs unfrozen (all frozen, or the server's unit stopped and its sessions unit
  not running unfrozen); no unit of it that does not run left processes that run
  (`freeze::left_running`); and `systemctl is-enabled` of the server's unit says `disabled`
  or `static` (`freeze::disabled`, which also takes `masked`). An unanswered question is not
  held.
- **"Finished" is said only by a run that finds the finish done and the old instance held.**
  Otherwise the run repeats the failed line (what it found, the commands, "The move is not
  finished until then"), ends with exit 1 and leaves that line in `status.json`, for
  `--status` and the page, with the phase `Cutover`.
- **A later run only looks.** The run that starts the new instance sends `systemctl disable`
  and, going on from a recorded start, freezes what runs beside a new instance that just said
  that it runs (SPEC-467). A run that finds the move recorded as moved changes nothing of the
  old instance; its line says that the new instance has taken over, not that it runs, names
  the abort as the way back, and names no command at all when systemd did not say what the
  units do.
- **The steps after the cutover never wait for the old instance.** The tasks, the redirect, the
  key and the firewall on the new host are done by the run that started the new instance and
  by a later run that finds the move not yet finished, whatever the look finds.

## Alternatives

- **Record "the old instance was held".** True of one moment; the defect is the case where a
  record and the host disagree. A unit thawed, started or enabled after the finish would again
  be answered with "finished".
- **Do not record `Moved` while the old instance is not held** (what SPEC-478 does for its own
  case, a stopped unit that left processes). The next run would go through the resumed start,
  which orders the start again when the old instance is frozen and, if the new instance does not
  answer as healthy at that moment, stops it and thaws the old one: a rollback of an instance
  that took over, perhaps days later. The firewall and the key on the new host would also wait
  for a person on the old one.
- **Do not record `finished` while it is not held.** Every later run would repeat the finish
  over a key that is gone and report "an earlier run deleted it" each time; `finished` would
  stop meaning what the abort and the finish read it for.
- **Let a later run freeze and disable.** It would act on a record: the new instance cannot be
  asked without the key, and a person who went back by hand (stopped the new instance, started
  the old) would have the only running instance frozen by a command they ran to look. The line
  names the commands, and the next run proves them.
- **Judge "disabled" by the exit code of `systemctl disable`.** It is a change, not a look, and
  says that a command ran, not what holds now.

## Consequences

- The same command after a failed line fails again until a person has run what the line names;
  scripts that took exit 0 of a second run for "done" now get exit 1 while two instances could
  run.
- A move that finished long ago is looked at whenever the command is run on the old host again;
  a wiped or retired old host is not asked (the command is not run there).
- `static` (a unit without `[Install]`) counts as disabled: nothing enables it, no `systemctl
  disable` changes the word, and a link made by hand makes it `enabled` (measured). Other
  states (`enabled-runtime`, `linked`, `indirect`, `alias`, …) count as "not known to be
  disabled", with systemd's word in the line. The unit `host setup` writes has an `[Install]`
  section, so `disable` leaves it `disabled`. A masked unit never reaches the look: the move
  refuses a unit that is not `loaded` before it.
- The old host's page reads the phase: `Cutover` with the failed line offers the abort, as for
  any move that is not finished.
