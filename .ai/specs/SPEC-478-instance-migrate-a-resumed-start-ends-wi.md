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
  whose stop left nothing has an empty `ControlGroup` at once. systemd 255 (Ubuntu 24.04)
  answers the same, and there a `systemctl stop` of the unit the move froze thaws it and stops
  it: one command by a person who sees a hanging move, and the old agents run unfrozen. systemd
  259 refuses that stop ("Cannot perform operation on frozen unit"), so there the state needs a
  thaw or a failed freeze first.
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

- [x] The defect reproduced: flow tests with a fake systemd that answers `ControlGroup` and `cgroup.events` fail on the unfixed code
- [x] A resumed start fails with the command to run while a stopped unit's processes run, and records nothing
- [x] The guide and the change log say what such a run does
- [x] The same run on a real host: an Ubuntu 26.04 guest with the build before and after the fix
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
  group), so this is the kernel's word taken as it is, covered by a unit test and, on the
  guest, by a group frozen by hand (`cgroup.freeze`).
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

## Evidence

Runs of 2026-11-12. The logs and scripts are kept in the task's ignored `target/evidence/`
while it runs; what they showed is quoted here.

- **The defect, by test.** At `5f9536e1` (the tests and the fake systemd alone, the code of
  `dev`) `cargo nextest run -p yulius --no-fail-fast -E 'test(/instance::/)'`: 327 run, 324
  passed, 3 failed: `a_recorded_start_with_a_stopped_server_whose_processes_run_does_not_end_well`,
  `a_recorded_start_names_every_stopped_unit_whose_processes_run` and
  `a_recorded_start_does_not_guess_what_a_stopped_unit_left`, each on `left: 0, right: 1` (exit
  0) with the `manual` line in the shown checklist. The fourth new flow test (nothing left: the
  line stays) passes before and after, as it must. At `2d373d6b` (the fix): 330 run, 330 passed.
- **Mutations** (`target/evidence/mutants.py`, at `2d373d6b`, each applied alone and undone):
  11 of 11 fail a test. The check removed from `resume_start` (the 3 tests above); `frozen`
  ignored; `populated` ignored; a running unit looked at too; the sessions unit left out; an
  unanswered systemd taken for "nothing left"; a `show` error swallowed in the helper; an unset
  `ControlGroup` read as the root group; the move recorded in the failed case; only the first
  unit named in the command; `KillMode=process` named for every unit.
- **What systemd answers** (QEMU guests from the Ubuntu cloud images; a unit with
  `KillMode=process`, `Delegate=yes` and a process in a subgroup):
  - Ubuntu 26.04.1, systemd 259.5: after `systemctl stop`: `ActiveState=inactive`,
    `ControlGroup=/system.slice/probe.service`, `cgroup.events` "populated 1 frozen 0";
    `systemctl freeze`: "Unit is not active", exit 1; `systemctl kill --kill-whom=all
    --signal=SIGKILL`: exit 0, then `ControlGroup=` and no group. A stop that ends everything
    (`KillMode=control-group`): `ControlGroup=` at once. `systemctl stop` of a frozen unit:
    "Cannot perform operation on frozen unit", exit 1. A frozen unit whose main process gets
    SIGKILL: `failed`, `FreezerState=frozen`, `ControlGroup=`, no group (nothing left).
  - Ubuntu 24.04.5, systemd 255.4: the same answers for a stopped unit with a process left and
    after the kill ("Unit is inactive." for the freeze). `systemctl stop` of a frozen unit
    succeeds there: `inactive`, `FreezerState=running`, "populated 1 frozen 0".
- **The real command on the 26.04 guest.** `yulius host setup --public-url http://vm.test
  --no-smoke` (exit 0: the real units, `yulius.service` in `yulius.slice` with
  `KillMode=process`, and `yulius-sandboxes.service`). Then: a process put into a subgroup of
  the running server's unit; a move's state with `"cutover":"Starting"` and its one-time key;
  `systemctl stop yulius.service`; a second server of the same build on the same listeners as
  the new instance. The new host is the guest itself, reached by the command over ssh to
  `root@127.0.0.1` with the staged build: one guest stands in for two, so everything on the old
  host's side is real (systemd, the cgroup files, `yulius instance migrate`), and the new host
  answers the health question for real but was not set up by a move. State before each run:
  `yulius.service` `inactive`, `ControlGroup=/yulius.slice/yulius.service`, "populated 1 frozen
  0", the process alive; `yulius-sandboxes.service` `active`.
  - Build of `dev` (`0dd7a58a`): `changed old instance yulius-sandboxes.service ran here …
    frozen again`, `manual old instance yulius.service does not run here any more, and the new
    instance runs. Make sure nothing …`, the finish, "failed 0", **exit 0**, state
    `"cutover":"Moved","finished":true`, with the process still running, "populated 1 frozen 0".
  - This change (`2d373d6b`), `target/evidence/guest/scenario-fix-2d373d6b2.log`: run 1: the
    sessions unit frozen again, then `FAIL old instance yulius.service has stopped, but
    processes of it still run here beside the new instance (a stopped server's unit keeps the
    agents, terminals and sandbox engines it started: KillMode=process), and systemd freezes
    nothing of a unit that has stopped. End them: \`systemctl kill --kill-whom=all
    --signal=SIGKILL yulius.service\`, then run the command again. The move is not recorded as
    done until then`, "failed 1", **exit 1**, state `"cutover":"Starting","finished":false`, the
    process alive. Run 2, nothing changed: the same line, exit 1. Then the named command (exit
    0; `ControlGroup=`, no group, the process gone). Run 3: the `manual` line, the finish,
    "failed 0", **exit 0**, `"cutover":"Moved","finished":true`.
  - Also with this change: `--status` after the failed run shows the `FAIL` line; with the
    group frozen by hand (`echo 1 > cgroup.freeze`: "populated 1 frozen 1") the run gives the
    `manual` line and exit 0; with an empty group left behind ("populated 0 frozen 0",
    `ControlGroup` still set) the same.
