---
yulius-task: 2d488ecd-4304-4dd1-88ed-926e238ac3a9
status: in-progress
---

# SPEC-478: A resumed start does not end well while processes of a stopped old unit run (issue #1178)

Status: in-progress
Date: 2026-11-12
Owner: Yulius task #460, issue #1178
Scope authorization: the request of task #460 (from task #458, which works through open issues):
fix issue #1178 in one pull request that says `Fixes #1178`, reproduce it first with a test that
fails without the fix, comment on the issue, open no new issue, and leave issue #1179 to its own
task (#461).

## Goal

`yulius instance migrate`, going on from a recorded start, no longer says that the old instance
"does not run here any more" and ends with exit 0 while agents, terminals or sandbox engines of
the stopped old unit still run beside the new instance. It says that they run, names the command
that ends them, fails, and looks again on the next run.

## Context

- **Symptom.** A run that finds `Cutover::Starting` (an earlier run ended after it ordered the
  new instance's start), the old server's unit `inactive` or `failed`, and the new instance
  reporting the build, printed a `manual` line ("… does not run here any more … Make sure
  nothing of the old one runs here: …"), recorded `Cutover::Moved` and ended with exit 0. The
  unit `host setup` writes has `KillMode=process` (`crates/yulius/src/host_setup.rs`), so a
  `systemctl stop` of it ends only the server: what the server started stays in the unit's
  cgroup and runs on, unfrozen, beside a new instance whose first start queued the same tasks
  again (SPEC-460 MIG-004: never two instances).
- **Root cause.** `resume_start` (`crates/yulius/src/instance/migrate.rs`) decided "nothing of
  it runs" from `Old::down`, which is built from `ActiveState` and `FreezerState` of the
  server's unit and its sessions unit alone. A unit that has stopped was never asked what it
  left: `ControlGroup` and the group's `cgroup.events` were not read. The export's freeze
  (`freeze::freeze`) and the new host's stop (`migrate/target.rs`) both read them; the resumed
  start, written later for another state (SPEC-467 LFT-002), did not, and SPEC-467 recorded the
  gap as this issue.
- **Why the tests missed it.** The flow tests' fake systemd (`migrate/tests/flow.rs`) modelled
  "stopped" as one flag, for a restarted host. It named a cgroup for every unit and answered no
  `cgroup.events`, so a stopped unit with processes could not be told from one without.
- **What real systemd says** (Ubuntu 26.04, systemd 259, a QEMU guest; Evidence): a unit with
  `KillMode=process` and a process in a subgroup, after `systemctl stop`:
  `ActiveState=inactive`, `ControlGroup` still set, `cgroup.events` "populated 1", "frozen 0";
  `systemctl freeze` fails ("Unit is not active"); `systemctl kill --kill-whom=all
  --signal=SIGKILL` ends them, and then `ControlGroup` is empty and the group is gone. A unit
  whose stop left nothing has an empty `ControlGroup` at once. systemd 259 refuses to stop a
  frozen unit ("Cannot perform operation on frozen unit"), so the state needs a thaw or a
  failed freeze before the stop; it is still one command away for a person who sees a hanging
  move.
- **Where sessions live.** On a host set up since SPEC-458 the agents live in the sessions unit
  (`<name>-sandboxes.service`), which a stop of the server's unit leaves `active`: the resumed
  start already freezes that one to stay (SPEC-467 LFT-002). The state of this issue is what
  stays in the server's own unit: everything on a host of the older layout, and whatever the
  server started outside the sessions unit on a newer one. The sessions unit has the default
  `KillMode`, so it leaves processes only while a stop of it has not ended them all.
- **Sibling, not this change:** issue #1179 (a move recorded as finished though the old unit
  could not be frozen and disabled; task #461, which starts after this one). This change records
  nothing in its failed case, so it does not depend on what #1179 decides, and
  `freeze::left_running` is there for it to use.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| RSL-001 | A resumed start that finds no unit of the old instance running unfrozen, a new instance that reports the build, and a stopped unit of the old instance (the server's, `inactive` or `failed`; its sessions unit when it does not run) whose cgroup holds processes that are not frozen (`cgroup.events`: `populated 1` without `frozen 1`) fails: a `FAIL` line names the units, says that processes of them still run beside the new instance, and names `systemctl kill --kill-whom=all --signal=SIGKILL <units>`; exit 1. The run kills, starts, stops, thaws and disables nothing, and the move stays recorded as `Starting`, so the same command asks again. A sessions unit that runs unfrozen is still frozen to stay first. | flow tests `a_recorded_start_with_a_stopped_server_whose_processes_run_does_not_end_well`, `a_recorded_start_names_every_stopped_unit_whose_processes_run`; unit tests of `freeze::left_running`; the guest |
| RSL-002 | With no cgroup (the unit left nothing, or the old host was restarted), an empty one, or one whose processes the kernel holds frozen, the `manual` line, `Cutover::Moved` and exit 0 stay as before. After the named command ended the processes, the same command goes on that way. | flow test `a_recorded_start_with_a_stopped_server_and_nothing_left_keeps_its_line` and the last part of the first test above; unit tests; the guest |
| RSL-003 | When systemd does not say what a stopped unit left, the run does not take that for "nothing": a `FAIL` line says so, exit 1, the move stays `Starting`. | flow test `a_recorded_start_does_not_guess_what_a_stopped_unit_left`; unit test `what_a_stopped_unit_left_is_not_guessed` |
| RSL-004 | `docs/operations.md` §7 ("Move the instance to another host") says what such a run does and what to run; `docs/changes.md` has the entry. | the documents |

## Plan

- [ ] The defect reproduced: flow tests with a fake systemd that answers `ControlGroup` and `cgroup.events` fail on the unfixed code
- [ ] A resumed start fails with the command to run while a stopped unit's processes run, and records nothing
- [ ] The guide and the change log say what such a run does
- [ ] The same run on a real host: an Ubuntu 26.04 guest with the build before and after the fix
- [ ] Every gate run and the independent review's findings answered

After the pull request is open and after its merge, not Plan items (`delivery_merge` needs every
step done first): its description says `Fixes #1178`, and issue #1178 is closed with a comment
that names the pull request.

## Acceptance criteria

- A resumed start with the old unit `inactive` or `failed`, its cgroup populated and the new
  instance reporting the build ends with exit 1 and a failed line that names what still runs
  and the command that ends it; nothing is recorded as moved.
- With no cgroup or an empty one the `manual` line and exit 0 stay.
- Flow tests for each case fail without the change; the fake systemd answers `ControlGroup` and
  `cgroup.events`.
- `docs/operations.md` §7 says what such a run does.
- The same is seen with the real binary on a real systemd.

## Test plan

- **Regression tests first** (`crates/yulius/src/instance/migrate/tests/flow.rs`), committed
  before the fix and failing on it: the fake systemd names no cgroup for a stopped unit that
  left nothing (as systemd does), answers `cgroup.events` for one that left a group, and can
  stop answering before the cgroup is asked for.
- **Unit tests of `freeze::left_running`** (`crates/yulius/src/instance/freeze/tests.rs`): each
  `ActiveState`, each answer of `cgroup.events`, the sessions unit, a silent systemd.
- **Mutations** (never committed): the check removed from `resume_start`; "frozen" ignored;
  the sessions unit left out; an error taken for "nothing left". Each must fail a test.
- **A real host**: an Ubuntu 26.04 cloud image as a QEMU guest. What systemd 259 answers in
  each state; then the real `yulius instance migrate` going on from a recorded start with the
  old unit stopped and a process left in its cgroup, once with the build of `dev` and once with
  this change, and again after the named command.
- **Gates**: `scripts/dev ci`; `yulius security-scan` (its state is recorded).

## Decisions

- **A failed line, not a kill.** The issue allows either. The run names the command and ends
  nothing itself. Reasons: (1) everywhere else a move leaves the old instance's processes
  alive, frozen, so that `--abort` can thaw it and nothing of its state is lost; a kill is the
  one thing here that cannot be taken back, and the processes are a person's agents and
  terminals on a host that a person stopped by hand for a reason the run does not know. (2)
  The freeze that the sibling state gets ("freeze, not only fail, when both run", SPEC-467) is
  not available: systemd freezes nothing of a unit that is not active (measured), and writing
  `cgroup.freeze` behind systemd's back would leave a freeze that no `systemctl thaw` and no
  abort undoes. (3) The command is one line, works in every state that was measured, and the
  next run proves that it worked. Alternative, killing in the run: rejected for (1); if it is
  wanted later it is a flag on top of this, not a different check.
- **Nothing is recorded in the failed case.** The state stays `Starting`, so the next run asks
  the new instance and systemd again and says the same until the processes are gone. Recording
  `Cutover::Moved` with a failed line (what the neighbouring "could not be frozen" case does)
  would make the next run say "finished" with exit 0: that is issue #1179, and this change does
  not add a case to it. The price: the steps after the cutover (the old URL's redirect, the
  finish) wait for that next run too. The new instance runs meanwhile either way.
- **Frozen processes do not count as running.** `cgroup.events` says `frozen 1` when the kernel
  holds everything in the group. Such processes do not run beside the new instance, which is
  what MIG-004 forbids, and the line would be false for them; the `manual` line still tells the
  person to make sure of them. systemd 259 did not produce that state in the probes (it
  refuses to stop a frozen unit, and a frozen unit whose main process is killed loses its whole
  group), so this is the kernel's word taken as it is, covered by a unit test.
- **Unreadable is empty, unanswered is not.** A group whose `cgroup.events` cannot be read is
  gone (systemd removes it with its last process; the export's freeze and the new host's stop
  read it the same way). A `systemctl show` that fails is no answer, and fails the run
  (RSL-003), as an unanswered question does elsewhere in the resumed start (SPEC-467 "unknown
  is treated as running").
- **The sessions unit is asked too.** It keeps the default `KillMode`, so it rarely leaves
  anything; a stop of it that is still on its way, or that could not end a process, is the
  same fact, and until now such a unit counted as ended. One helper asks both units.
- **One reader of `cgroup.events` in `freeze.rs`.** The export's freeze had the read inline; it
  now shares `group()` with `left_running`. Its behaviour is unchanged (it refuses on
  `populated 1` whatever `frozen` says). The new host's stop in `migrate/target.rs` reads
  through another `Host` trait and stays as it is.
- **No new ADR.** ADR-0297 (one resumable checklist that freezes the old instance and thaws it
  when the move does not happen) stands and this follows it; the choice above is this spec's,
  as the issue asks.
