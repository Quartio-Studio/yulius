---
yulius-task: 6c1a1301-0997-4895-b6fb-112ea0720a93
status: in-progress
---

# SPEC-467: Instance move: the last review's leftovers (two minors, five nits)

Status: in-progress
Date: 2026-11-11
Owner: Yulius task #450 (runs after #442)
Scope authorization: task #450, asked by the project's owner: "Fix the findings the independent
review left open on `yulius instance migrate` (task #442, PR #1169, SPEC-460, ADR-0297) … fix
each test-first".

## Goal

The seven findings the last review of `yulius instance migrate` left open are fixed, each with a
test that fails without the fix: the move never leaves the old unit frozen behind a line that
says it does not run, never ends with exit 0 while both instances run, finishes a move whose
finish was done but not recorded, proves a copy whose old host still holds a limits mark, and its
messages and documents say what the code does.

## Context

- SPEC-460 (task #442, PR #1169) delivered the command; its last review approved the head
  `8e73f8a4` without a blocker or major and listed two minors and five nits, split off into this
  task so the approved head could merge. This branch starts from that head.
- Where the findings are (all in `crates/yulius/src/instance/`):
  1. `migrate.rs`, `cutover`: after `freeze()` succeeded one `systemctl show` decides
     (`old_frozen()`). When it fails, `claim(held)` gives the freeze's end to the move
     (`leave_frozen`), nothing thaws, and the line says "is not running any more: start it"
     about a unit that is frozen.
  2. `migrate.rs`, `resume_start` (a recorded `Cutover::Starting`): with the old unit running
     unfrozen (`old_unit()` is `Some((true, false))`) and a new instance that reports the build,
     the run records `Moved` and exits 0 with a `manual` line while both instances run.
  3. `migrate.rs`, `finish`: only "Permission denied" counts as "the key is gone". After a finish
     that ran but could not save `finished`, the key file is deleted on the old host, the
     connection says "there is no one-time key yet", and every run ends in "Run the command
     again". `after()` accepts both wordings.
  4. `walk.rs`, `inventory.rs`: a `.yulius-limits-adapt` left on the old host (an import whose
     first start never consumed it, SPEC-459 HSL-005) is exported and counted; the import
     replaces it with its own (`import.rs`, `limits_mark`), which the new host counts too, with
     another time. The proof fails after the whole copy.
  5. `docs/decisions/README.md` lists 0297 before 0294 (the union merge put dev's lines after it).
  6. `docs/operations.md` §7 says a run that hangs after the start was ordered ignores Ctrl-C.
     That holds for the run that froze the unit (its signal thread is armed); a later run that
     goes on from the recorded start arms nothing, so Ctrl-C ends it (nothing thaws).
  7. `migrate.rs`, `resume_start`: "the new host does not answer" and "the new instance does not
     run" share one line that points to `--abort`.
- Tasks #445 (warm copy), #446 (GitHub App check, remote CLI) and #447 (the button) change the
  same files in parallel; `origin/dev` is merged on a conflict.

## Requirements

- LFT-001 **A look that fails right after the freeze thaws.** In the cutover, when the unit is
  not known to be frozen right after the freeze, nothing is copied and the line says what systemd
  said: the unit does not run (as before: "is not running any more: start it"), it runs unfrozen
  (thawed from outside), or systemd does not say. In the last two cases the move thaws the unit
  after it claimed the freeze's end, so the old instance runs on; the copy on the new host is not
  touched and the same command starts over. (Amends SPEC-460 MIG-004: "a failure … before the new
  instance is started thaws the old one".)
- LFT-002 **A recorded start never ends in two running instances.** A run that goes on from a
  recorded start and finds the old unit not frozen asks the new instance, as before. When it
  reports the build:
  - an old unit that does not run stays a `manual` line (make sure nothing of it runs) and the
    move is recorded as done;
  - an old unit that runs unfrozen, or whose state systemd does not say, is frozen by the run,
    to stay (bounded by the freeze's wait), and then disabled and checked as after any start.
    When it is frozen the checklist says that it ran beside the new instance (`changed`); when
    it cannot be frozen the line is `FAIL` with the reason and the commands, the move is still
    recorded as done (the new instance has taken over, MIG-004) and the run exits 1. Nothing
    thaws on that path: a freeze that systemd refuses or that is not complete within the wait is
    left as it stands (SPEC-460 MIG-004: once the start was ordered nothing thaws the old
    instance).
- LFT-003 **The new host's silence is not "does not run".** A run that goes on from a recorded
  start and cannot reach the new host with the move's key (no connection, no answer) fails
  saying so, with `ssh`'s reason, and tells to run the command again when the new host answers
  (the abort only once the new instance is known to be stopped). It does not ask for the key
  again: the run that ordered the start had added it; only a key that is missing on the old
  host or refused by the new one is made and added as in a plan. The same holds when the host
  is reached and the question about the new instance gets no answer. A new instance that
  answers and does not report the build keeps the line that points to `--abort`. None of these
  starts, stops, freezes or thaws anything, and the recorded start stays.
- LFT-004 **A finish that was done is found done.** `finish` takes a key that no longer exists
  on the old host for a finish that was done: only a finish deletes it, once the new host's
  finish answered. The finish is recorded (`finished`), the checklist says so (`manual`: look at
  the new host's firewall once, and what to remove there if the key was deleted by hand instead)
  and the run ends with exit 0, whether or not the new URL answers from the old host (a DNS
  record may still be open). A key the new host refuses stays as it was (done when the new
  instance answers); a new host that cannot be reached with a key that still exists keeps "Run
  the command again".
- LFT-005 **The import's limits mark is never the copy.** `yulius instance export` leaves a
  top-level `.yulius-limits-adapt` out, whatever it is (file, link, directory), and neither host
  counts it: every import writes its own (SPEC-459 HSL-005). A move from an old host that holds
  the mark proves its copy; the new host has the import's mark. An archive that still brings the
  name (an older build's) is installed as before: replaced, a link never followed. (Amends
  SPEC-329 EXP-006.)
- LFT-006 **Documents.** `docs/decisions/README.md` lists 0297 after 0296; `docs/operations.md`
  §7 says which run ignores Ctrl-C (the one that ordered the start) and that a later run from
  the recorded start ends at Ctrl-C without thawing anything, and describes LFT-001 to LFT-005
  where it describes the move.

## Plan

- [x] Spec SPEC-467 for the seven findings
- [x] A failed look right after the freeze thaws the unit and says what is known (finding 1)
- [x] A recorded start freezes an old unit that runs beside the new instance, and a silent new host is told from one that does not run (findings 2 and 7)
- [x] A finish that was done but not recorded is found done by the missing key (finding 3)
- [x] The limits mark is left out of the export and of both counts (finding 4)
- [x] ADR index in order, the guide's Ctrl-C paragraph true, changes entry (findings 5 and 6)
- [x] A real move between two QEMU guests through the fixed paths
- [ ] A freeze beside a running new instance never thaws, and a new host that cannot be reached is waited for
- [ ] Gates green and the independent review answered

## Acceptance criteria

- With `systemctl show` silent from the look right after the freeze: exit 1, a `FAIL` line that
  says systemd does not say whether the unit is frozen (not "is not running any more"), the unit
  thawed and still enabled, nothing copied, the cutover still `No`; with systemd answering again
  the same command completes. Thawed from outside at that look: the line says it runs unfrozen,
  nothing is copied. Stopped at that look: the line and the outcome as before.
- A recorded start, the old unit running unfrozen, the new instance reporting the build: the run
  freezes and disables the old unit, records `Moved`, shows no "Make sure nothing of the old one
  runs" line, and finishes with exit 0. The same with a freeze that systemd refuses, and with one
  that never completes: exit 1, a `FAIL` line with the reason, `Moved`, and no `systemctl thaw`.
  With an old unit that does not run: the `manual` line, `Moved`, exit 0, and nothing frozen,
  started or stopped.
- A recorded start and a new host that cannot be reached, with the old unit frozen or not: exit
  1, a line with `ssh`'s reason and "run the command again when the new host answers", no
  sign-in and no "add this line", nothing done on either host, the state still `Starting`; once
  it answers the same command completes. A key the new host refuses is added again and the run
  goes on.
- A recorded start, the old unit not frozen, the new host reached but not answering the question:
  exit 1, a line with "run the command again when the new host answers", no `systemctl` verb on
  the old host, the state still `Starting`. With a new instance that answers and does not run:
  the line with `--abort`, as before.
- A move whose finish ran, with `finished` not saved: the next run ends with exit 0, says that the
  finish on the new host was done and records `finished`, with the new URL answering or not.
- An old data directory with a `.yulius-limits-adapt` of another time: the export's plan does
  not list it, both inventories are equal, and the installed copy has the import's own mark. The
  import of a crafted archive that brings a link at that name still replaces it without
  following it.
- The ADR index lists 0296, 0297, 0299 in that order with every record once; the repository
  check passes.

## Test plan

- Flow tests (`instance::migrate::tests::flow`, two fake hosts, the real export and extraction):
  one per acceptance scenario of LFT-001 to LFT-004. The fake systemd gets "after n looks since
  the freeze, this happens to the unit", a freeze that is refused and one that never completes;
  the fake connection says "there is no one-time key yet" once its key was deleted and `ssh`'s
  words for a host that cannot be reached, as the real one does.
- Freeze tests (`instance::freeze::tests`): the freeze to stay never thaws and asks no helper.
- Inventory test (`instance::inventory::tests`): the leftover mark, through the real export and
  import. Walk test: the mark is not listed at the top and is kept below it. Import test: the
  crafted archive with a link at the mark's name.
- Gates: `scripts/dev ci`; `yulius security-scan` (Vidoc is off for this project: disabled, not
  passed).
- Real hosts: two Ubuntu 26.04 guests (QEMU), as in SPEC-460's Evidence: a move from an old host
  with a leftover mark; a run killed after it ordered the start, the old unit thawed by hand, the
  same command again; a finish whose record is taken back.
- Not testable here: a `systemctl show` that fails for exactly one call on a real host (the fake
  systemd does it).

## Decisions

- **Freeze, not only fail, when both run (LFT-002).** The review allowed either. The new instance
  has taken over once it reports the build (MIG-004), so the old one must stop running, and the
  run can do that itself at once; a `FAIL` line alone would leave both running until a person
  reads it. When the freeze fails, the line is the `FAIL` the review asked for.
- **Not the export's freeze (LFT-002).** The first version of this fix called `freeze::freeze`.
  Its failure paths thaw: a freeze that takes longer than the wait, a `systemctl freeze` that
  fails, an update helper that became active. That is right for an export and before a start is
  ordered (a failed freeze must leave the instance running) and wrong beside a new instance that
  has taken over: the more of the old one is frozen the better, and SPEC-460 lets nothing thaw
  after the start was ordered. Its refusals are also worded for an export ("start it so the
  export can freeze them"), which here would tell a person to start the old instance beside the
  new one. `freeze::freeze_to_stay` sends the freeze, waits the same bound and never thaws; it
  does not ask the update helper, which every run already refuses to start beside.
- **Unknown is treated as running (LFT-002).** When systemd does not say what the old unit does,
  "it does not run" would be a guess that ends in exit 0; the run tries the freeze, and the check
  after it fails closed.
- **An unreachable new host is said before the key is asked for (LFT-003).** With the real
  connection a host that is down fails the key's check, which comes before the question about
  the new instance: the run then tried the operator's sign-in again, printed "add this line to
  authorized_keys" and ended with exit 0, with the old instance frozen and nobody knowing what
  the new one does. In a run that goes on from a recorded start the key was added already, so
  only a missing or refused key is a key matter; anything else is the host, and is waited for.
  A plan's first sign-in is unchanged.
- **The mark is left out of the export, not only out of the counts (LFT-005).** One rule
  (`walk::left_out`) for the export's walk and for the copy's count: what is not sent cannot be
  counted differently, and a directory or link at that name on the old host cannot fail the
  import after the whole copy either. The import keeps replacing what an archive brings there, for
  archives of older builds. The name is not added to `LEFT_OUT`: that list is also what may sit
  in a target before an import whatever its kind, and the mark may be there only as a file.
- **A key deleted here needs no answer from the new URL (LFT-004).** The key the new host
  refuses is evidence from outside, so the new instance's answer is asked for with it. The key's
  absence on the old host is this host's own record: `forget` runs only after the new host's
  finish answered (or in an abort, which leaves no `Moved` state). Asking the URL too would keep
  the run failing for as long as the DNS record is open, about a finish that is done.
- **The guide is made true, not the code (finding 6).** A resumed run that ends at Ctrl-C thaws
  nothing and is taken up by the same command; making it ignore the signal would only take the
  operator's way out of a hanging run. The run that froze the unit keeps ignoring signals once
  the start is ordered: its signal thread exists to thaw before that point and must do nothing
  after it.
- **No new ADR.** ADR-0297's decisions stand (the start is the point of no automatic return; the
  inventory is the proof); these are corrections inside them.
