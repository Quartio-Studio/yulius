---
yulius-task: fedcfccc-a17b-4ef0-b435-c6fa39b5e17e
status: done
---

# SPEC-481: A move is not finished while its old instance is not held (issue #1179)

Status: done
Date: 2026-11-12
Owner: Yulius task #461, issue #1179
Scope authorization: the request of task #461 (from task #458, which works through open issues):
fix issue #1179 in one pull request that says `Fixes #1179`, reproduce it first with a test that
fails without the fix, comment on the issue, open no new issue.

## Goal

`yulius instance migrate` no longer says "finished" with exit 0 for a move whose old instance is
not known to be held: frozen as a whole, or stopped with nothing of it left running, and
disabled. Every run of a move whose new instance has taken over looks at the old instance again,
says what a person must do while it is not held, ends with exit 1, and `--status` shows that
line until it holds.

## Context

- **Symptom.** A run whose new instance reports the build while the old unit cannot be frozen
  or disabled prints `FAIL old instance … must be frozen and disabled now …` and ends with exit
  1, once. The same command again prints `ok move finished: the instance runs on …` and ends
  with exit 0; `status.json` holds that checklist, so `--status` shows no failure either. The
  old instance may run beside the new one, or start beside it when its host restarts (SPEC-460
  MIG-004: never two instances).
- **Root cause.** `moved()` (`crates/yulius/src/instance/migrate.rs`) printed the failed line
  and recorded `Cutover::Moved` all the same; `steps()` went on to `finish()`, which recorded
  `finished`; and a run that found `finished` returned "finished" at once. Nothing after the
  first look ever asked systemd about the old instance again: a state the run had seen and
  reported was the only record of it, and that record was a line of a checklist the next run
  overwrote. The same holds for a move that was held when it finished and is not any more (a
  unit thawed or started by hand, `systemctl stop` of the frozen unit on systemd 255, which
  thaws it and leaves its agents running): no later run looked.
- **Why the tests missed it.** The flow tests of the failed line
  (`an_old_unit_that_cannot_be_disabled_is_thawed_and_nothing_starts`,
  `a_recorded_start_never_ends_with_both_instances_running`) assert exit 1 and
  `Cutover::Moved` and never run the command a second time. The fake systemd answered no
  `is-enabled`, and no code asked.
- **Ways into the failed line**: `systemctl disable` failing after any start; a unit of the old
  instance thawed from outside between the start and the look; a resumed start whose freeze of
  a unit that runs beside the new instance fails (SPEC-467 LFT-002).
- **What stays** (SPEC-467 LFT-002, SPEC-460): once the new instance reports the build it has
  taken over. The move is recorded as moved, nothing is rolled back, and the steps after the
  cutover (the public URL, the tasks, the old URL's redirect, the finish with the key and the
  firewall) are done whatever the old unit does: they are the new instance's.
- **Sibling**: issue #1178 (SPEC-478, task #460), whose branch this one started from and whose
  pull request #1232 merged while this one was written. It added `freeze::left_running` (what a
  unit that does not run left in its cgroup), which the look here uses. Its own failed case
  records nothing and is left as it is.
- **Asked along by the issue and its comments**: the line of `start()` for a new host that
  stopped answering names `--abort` without saying when it is safe; `finish()` takes a key the
  new host refuses for "done there" only when the new URL answers, and no test holds the second
  half; a comment gives a weak reason for not trying a step over a missing key.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| HLD-001 | **Held.** The old instance of a move whose new instance has taken over is held when systemd says all of this: no unit of it runs unfrozen (the server's unit is frozen and its sessions unit, where it runs, is frozen; or the server's unit has stopped, `inactive` or `failed`, and its sessions unit does not run unfrozen); no unit of it that does not run has left processes that run (`freeze::left_running`); and `systemctl is-enabled` of the server's unit says `disabled`, or `static` for a unit without an `[Install]` section (`freeze::disabled` also takes `masked` and `masked-runtime`, which the command never meets: a masked unit is not `loaded` and the move refuses it before the look). An unanswered question is not held. | unit test `only_a_unit_systemd_says_has_no_enablement_is_disabled`; flow test `a_unit_without_an_install_section_is_held_once_its_link_is_gone`; the flow tests below; the guest (G) |
| HLD-002 | **Every later run looks.** A run that finds the move recorded as moved, finished or not, asks systemd as in HLD-001 before anything else of the move, and changes nothing of the old instance: it freezes, thaws, stops, starts, enables and disables nothing. Held: an `ok` line under the heading `Cutover` says how (frozen and disabled, or does not run and is disabled). Not held: a `FAIL` line there says what was found, that the old instance must be frozen and disabled now that the new instance has taken over (not "runs": this run did not ask it), the commands, for a unit that runs unfrozen the way out of a freeze that keeps failing (HLD-009, with the disable), that the move is not finished until then, and the way back (`yulius instance migrate --abort`, once the new instance is stopped). For a stopped unit that left processes the line names the kill, for a stopped unit that is not disabled the disable. When systemd does not say what the old units do, nothing is known to be wrong: the line says so and names no command but the same one again. The run ends with exit 1 and no "finished" line is printed. The phase of `status.json` is `Cutover` after every run whose look failed, also the run that started the new instance or did the finish, and `done` again once a run finds the instance held. | flow tests `a_move_whose_old_unit_was_not_held_is_not_finished_until_it_is`, `a_finished_move_whose_old_instance_no_longer_holds_fails_until_it_does`, `a_later_run_looks_at_the_sessions_unit_too`; the guest |
| HLD-003 | **The finish is still done.** A run that finds the move moved and not finished goes on to the steps after the cutover and to the finish whether or not the old instance is held, as the run that started the new instance does. `finished` in the state file keeps its meaning (the finish was done: the key is gone); "finished" is said, with exit 0, only by a run that finds the finish done and the old instance held. | flow tests `a_moved_move_is_finished_there_and_not_called_finished_while_the_old_unit_runs`, `a_finish_that_did_not_reach_the_new_host_is_tried_again`; the guest (F) |
| HLD-004 | **The look after a start is the same look.** `moved()` sends `systemctl disable` as before and then decides by HLD-001: the failed line when the instance is not held, also when a unit of it that does not run left processes. A server's unit that has stopped with nothing left is said as that, not as "stays frozen". | flow test `a_unit_stopped_while_the_new_instance_started_is_asked_what_it_left`; the existing tests of the failed line; the guest (A) |
| HLD-005 | **A stopped unit must be disabled too.** A resumed start that finds the old server's unit stopped with nothing of it left and the new instance running keeps its `manual` line and records the move; when the unit is not known to be disabled a `FAIL` line names `systemctl disable <unit>`, the run ends with exit 1, and later runs look (HLD-002). | flow test `a_recorded_start_with_a_stopped_server_that_is_not_disabled_fails_until_it_is` |
| HLD-006 | **`--abort` is named with its condition.** The line of `start()` for a new host that stopped answering says "`yulius instance migrate --abort` only once the new instance is known to be stopped", as the lines of SPEC-467 LFT-003 do. | flow test `a_lost_answer_to_the_start_is_decided_by_asking_the_new_host` |
| HLD-007 | **A refused key with a silent URL is not "done there".** Held by a test (the behaviour is unchanged): the key stays on the old host, nothing is recorded as finished, and the run says to run the command again. | flow test `a_refused_key_with_a_silent_url_is_not_a_finish_found_done` |
| HLD-008 | `docs/operations.md` §7 ("Move the instance to another host") says what a later run does; `docs/changes.md` has the entry; ADR-0311 records what "finished" means. | the documents |
| HLD-009 | **A frozen unit is never named to be stopped or killed.** The commands a line names for a freeze that keeps failing (`systemctl stop …; systemctl kill --kill-who=all --signal=SIGKILL …`) name only units that are not frozen: in a later run's line the units systemd says are not frozen, in the resumed start's line (SPEC-467 LFT-002) the units whose freeze failed. The resumed start's "make sure" line for a stopped server names that unit alone: its sessions unit is frozen there. | flow tests `a_later_run_looks_at_the_sessions_unit_too`, `a_recorded_start_freezes_whatever_unit_runs_beside_the_new_instance`, `a_recorded_start_with_a_stopped_server_freezes_its_sessions`; the guest (H, I) |

## Plan

- [x] The defect reproduced: flow tests that run the command again after the failed line fail on the unfixed code
- [x] One look decides whether the old instance is held, after a start and in every later run of a moved or finished move
- [x] A stopped unit that is not disabled fails, the abort line names its condition, and the finish's refused-key case is pinned
- [x] The guide, the change log and ADR-0311 say what a later run does and what finished means
- [x] The same runs on a real host: an Ubuntu guest with the build before and after the fix
- [x] Every gate run and the independent review's findings answered

After the pull request is open and after its merge, not Plan items (`delivery_merge` needs every
step done first): its description says `Fixes #1179`, and issue #1179 is closed with a comment
that names the pull request.

## Acceptance criteria

- After a run that ended with `FAIL … must be frozen and disabled now`, the same command looks
  at the old unit again: still running or not disabled gives the failed line again and exit 1;
  frozen (or stopped with nothing left) and disabled gives "finished" and exit 0.
- `yulius instance migrate --status` shows the failed line until then.
- Flow tests for both in `crates/yulius/src/instance/migrate/tests/flow.rs`, failing without
  the change.
- The `start()` line for a new host that stopped answering says when `--abort` is safe.
- `docs/operations.md` §7 says what a later run does.
- The same is seen with the real binary on a real systemd.

## Test plan

- **Regression tests first** (`migrate/tests/flow.rs`), committed before the fix and failing on
  it: the fake systemd answers `is-enabled` from the state its `disable` and `enable` set; the
  tests run the command again after the failed line, with the old unit still not held and then
  held, for each way into the line (a disable that fails, a freeze that fails, a unit thawed
  after a finished move, a stopped unit that left processes, an unanswered systemd), on a host
  with and without a sessions unit, and read `status.json`.
- **Unit tests of `freeze::disabled`** (`freeze/tests.rs`): each answer of `is-enabled` that
  counts, those that do not, an exit code without a word, a `systemctl` that does not run.
- **Mutations** (never committed, `target/evidence/mutants.py`): the look removed from the
  finished branch; from the moved branch; the disabled question dropped; the leftover question
  dropped; "enabled" or no word taken for disabled; `masked` not taken; an unanswered systemd
  taken for held; a stopped unit with nothing left not held; the later look made to disable or
  freeze; the finish skipped when the look failed; the wording and the phase cases. Each must
  fail a test.
- **A real host**: an Ubuntu cloud image as a QEMU guest with a real instance (`yulius host
  setup`). What `systemctl is-enabled` answers in each state; then the real `yulius instance
  migrate` with the build before and with this change: a recorded start whose `systemctl
  disable` fails for real, a finished move whose old unit was thawed, stopped with a process
  left or enabled, and a move that is not yet finished; each again once it is held.
- **Gates**: `scripts/dev ci`; `yulius security-scan` (its state is recorded).

## Decisions

- **"Finished" is said, not stored** (ADR-0311). The state file keeps `cutover: Moved` and
  `finished` with their meanings: the new instance has taken over, and the finish was done (the
  key is gone on both hosts). Whether the move is finished for a person is decided by every run
  from what systemd says now. Alternatives: (1) a third recorded fact, "the old instance was
  held": it would be true of one moment, and the state this issue is about is the one where a
  recorded fact and the host disagree (a unit thawed, started or enabled later); (2) not
  recording `Moved` while the old instance is not held, as SPEC-478 does for its case: the next
  run would then go through the resumed start, which may order the start again and, should the
  new instance not answer as healthy at that moment, stop it and thaw the old one: a rollback of
  an instance that has taken over, possibly days later, and the steps after the cutover (the
  firewall on the new host, the key) would wait for a person on the old host; (3) not recording
  `finished` while it is not held: every later run would repeat the finish over a key that is
  gone and say "an earlier run deleted it" each time.
- **A later run looks and touches nothing.** The first look (`moved()`) follows a start this
  command ordered, with the new instance just asked. A later run cannot ask it once the key is
  gone, and a person may have gone back by hand: freezing or disabling an old instance on a
  recorded guess could stop the only one that runs. The line names the commands; the next run
  proves that they worked. Alternative, the later run freezes and disables as the resumed start
  does: rejected for that reason; the resumed start freezes only after the new instance said
  that it runs.
- **`is-enabled`, not the exit code of `disable`.** A look must not change anything, and
  `systemctl disable` exiting 0 says that a command ran, not what holds now. `disabled` and
  `static` are the answers that mean "nothing enables it" (and `masked`, below); every other
  answer, an empty one and an error are "not known to be disabled", and the line quotes what
  systemd said. `moved()` still sends `disable` first and then decides by the same
  look, so one rule holds after a start and in every later run.
- **The steps after the cutover do not wait for the old instance.** They are the new
  instance's (its firewall, its key, its tasks), and the look needs no key. So a run that fails
  the look still finishes, as the run that started the new instance always did.
- **The phase of a run whose look failed is `Cutover`.** The page offers the abort while the
  phase is not `done` (`yulius-app::migration`), which is right for a move that is not finished;
  and the old instance held is the cutover's own promise. That holds for the run that started
  the new instance and for the one that did the finish too, although they went on to the
  finish: on the guest the first run's `--status` read "done" above its `FAIL` line and the
  second run's read "Cutover", and one of them was wrong. A failed line that is not the old
  instance's (the firewall on the new host) leaves `done`, as before.
- **"Is not frozen as a whole", for every state that is not held by a freeze or a stop.** The
  line does not say which unit runs: a unit on its way up or down, a server that has stopped
  beside sessions that run, and a thawed pair all get the two commands and, in a later run, the
  way out. On the guest `systemctl freeze` of both units with the server's stopped exits 1
  ("Unit is not active") and freezes the sessions unit all the same, so the one line works in
  each of them.
- **`static` counts as disabled (review, minor 2).** A unit without an `[Install]` section
  answers `static`, with exit 0, and no `systemctl disable` changes that word, so a line that
  names the disable would be a dead end and a healthy move on such a host would never read
  finished (before this change it did: `disable` exits 0). Measured: such a unit wanted through
  a link made by hand answers `enabled`, and `disable` removes the link and leaves `static`. So
  `static` says what `disabled` says: nothing enables the unit. Alternative, refusing it with a
  line that tells the person to add an `[Install]` section: rejected, the unit is as held as
  one can be. `enabled-runtime`, `linked`, `indirect` and `alias` stay "not known".
- **`masked` is taken by the function and never met by the command (review, minor 3).** A
  masked unit does not start, and `is-enabled` says `masked` or `masked-runtime` (measured with
  a vendor unit). But a masked unit has `LoadState=masked`, and the move refuses a unit that is
  not `loaded`, or whose file is not under `/etc/systemd/system` (where `systemctl mask`
  refuses, "already exists"), before the look (`Profile::read`). So the guide names `disabled`
  and `static` only; `freeze::disabled` keeps the word, for it is true of systemd. Measured
  too: `systemctl disable` exits 0 for a masked unit without removing its enablement, for a
  unit enabled with `--runtime` (it stays `enabled-runtime`) and for a unit without
  `[Install]`: the exit code of `disable` is not the answer, which is why the look reads
  `is-enabled`.
- **A later run says "has taken over" and names the abort (review, minor 5).** It did not ask
  the new instance. In the case this spec itself names (a person went back by hand), a line
  that says "the new instance runs" and names a freeze would have the only running instance
  frozen. The line states what is recorded and names both ways: hold the old instance, or go
  back with `--abort` once the new instance is stopped. The run that started the new instance
  had just asked it and keeps "runs".
- **An unanswered systemd names no command in a later run (review, minor 4).** Nothing is known
  to be wrong, and a freeze, a stop and a kill are not what to do on no information. The line
  says that systemd did not answer and to run the command again, as the leftover question does.
  After a start the line keeps the two commands (SPEC-467 "unknown is treated as running"): the
  new instance had just been started.
- **The way out of a failing freeze names only units that are not frozen, also in SPEC-467's
  line (review, P1, pre-existing there).** The line named every unit of the old instance.
  Measured on the guest with the real `yulius.service` frozen (systemd 259.5): `systemctl stop`
  is refused ("Cannot perform operation on frozen unit"), `systemctl kill --kill-who=all
  --signal=SIGKILL` exits 0, and the unit is left `activating (auto-restart)`,
  `FreezerState=frozen`, without a cgroup; `systemctl thaw` says "Unit is not active", and
  `start` and `enable --now` (what the abort sends) are refused until the host restarts. A
  frozen unit is held as it is, so naming it gains nothing and can cost the way back. This
  change would have repeated the line in every later run, so both places now go by one rule;
  the later run's way out also ends with the disable (review, nit 6), since a person takes it
  instead of the two commands. The third place (confirming round, minor 2) is the resumed
  start's "make sure" line for a stopped server, which named the sessions unit beside it: that
  unit is frozen there. Measured with the real `yulius-sandboxes.service` frozen: `systemctl
  stop` is refused, the kill exits 0 and leaves it `failed` and `FreezerState=frozen`, thaw and
  start are refused, and only `systemctl reset-failed` lets it start again. The line now names
  the stopped server's unit alone.
- **The look's line stands under the heading `Cutover`, held or not (review, nit 12).**
- **What the review saw in code that was there before and this change leaves** (written in a
  comment on issue #1179, with what was measured): a server's unit whose file in
  `/etc/systemd/system` is a symlink passes the profile, and the cutover's `systemctl disable`
  removes that link too, which leaves a frozen unit systemd no longer knows. It is the
  cutover's disable since SPEC-460 and the profile's rule for what unit a move carries, not
  what a later run says. The weak reason for not trying a step over a missing key (`ssh` "can
  only complain") is replaced by the true one in the code's comment, two test comments and
  SPEC-467 LFT-004, as the issue's comment asked.
- **ADR-0311 follows 0309 without a 0310**: the number was given by `record_number`, which
  reserves across parallel tasks.
- **SPEC-478's failed case is left as it is** (nothing recorded, the move stays `Starting`): it
  is safe either way, it merged as it is (#1232), and changing it is not this issue.

## Evidence

Runs of 2026-11-12. Logs and scripts are kept in the task's ignored `target/evidence/` while it
runs; what they showed is quoted here. The code's last state is `ab9b0459`, the documents'
`73b04f48`; this section is committed after them. The independent review's first round, at
`742779e7`, and its confirming round, at `ef16e0d0`, are recorded below with their answers;
its look at the answers of the confirming round comes after this commit.

- **The defect, by test.** At `df0600a4` (the tests and the fake systemd's `is-enabled` alone)
  `cargo nextest run -p yulius --no-fail-fast -E 'test(/instance::/)'`: 338 run, 331 passed, 7
  failed. Five fail on the defect itself, `left: 0, right: 1` (exit 0 where the old instance is
  not held): `a_move_whose_old_unit_was_not_held_is_not_finished_until_it_is`,
  `a_finished_move_whose_old_instance_no_longer_holds_fails_until_it_does`,
  `a_later_run_looks_at_the_sessions_unit_too`,
  `a_moved_move_is_finished_there_and_not_called_finished_while_the_old_unit_runs`,
  `a_recorded_start_with_a_stopped_server_that_is_not_disabled_fails_until_it_is`. Two fail on
  a line: `a_lost_answer_to_the_start_is_decided_by_asking_the_new_host` (the abort's
  condition) and `a_unit_stopped_while_the_new_instance_started_is_asked_what_it_left` (a
  freeze named for a stopped unit). The eighth new test,
  `a_refused_key_with_a_silent_url_is_not_a_finish_found_done`, passes before and after, as it
  must (HLD-007). At `f4c95011` (the fix, with the unit test of `freeze::disabled`): 339 run,
  339 passed. The phase: at `2397655e` (assertions alone) 3 failed on `left: "done", right:
  "Cutover"`; at `4cbac2e2`: 339 passed. The review's findings the same way: at `a7ec97de`
  (tests alone) 340 run, 331 passed, 9 failed (the unit test of `freeze::disabled`, the new
  `a_unit_without_an_install_section_is_held_once_its_link_is_gone`,
  `a_recorded_start_freezes_whatever_unit_runs_beside_the_new_instance` and six of the tests
  above); at `2f7c4882`: 340 run, 340 passed. The confirming round's: at `fd57d9a4` (tests
  alone) 340 run, 339 passed, 1 failed
  (`a_recorded_start_with_a_stopped_server_freezes_its_sessions`, on the "make sure" line; the
  three cases that pin the way out pass there, as pins do); at `ab9b0459`: 340 run, 340
  passed.
- **Mutations** (`target/evidence/mutants.py` at `2f7c4882`, each applied alone and undone): 41
  of 41 fail a test (one of them written wrongly at first, so that it did not compile; run
  again alone once corrected). They are the 27 of the round before the review (27 of 27 then;
  the first run of all had 23 and one survivor, pinned by `7135c48f`), reworded where the code
  changed, and those the review's findings add: `is-enabled` asked of another unit, in the look
  and in the resumed start; the last line of the answer read; `static` not taken; an
  unanswered later look that names the commands; a later run that says "runs"; one that names
  no way back; the run after a start naming the way back; a frozen unit that is not disabled
  told "runs" in a later run; the way out naming a frozen server, leaving out the sessions
  unit, or lacking the disable; the resumed start's way out naming every unit; the look said
  under the plan. The reviewer's own 18 mutants: 14 killed at `742779e7`, 4 survived there, and
  each of the 4 is among the 41. At `ab9b0459`, 5 more, 5 of 5 killed
  (`target/evidence/mutants-round3.log`; one again written wrongly at first): the four of the
  reviewer's six on the delta that had survived at `ef16e0d0` (the way out leaving out a server
  that is not active; naming frozen sessions; the server read as frozen with its sessions; the
  resumed start's way out naming only the first unit) and the "make sure" line naming the
  frozen sessions unit.
- **What systemd answers** (a QEMU guest from the Ubuntu cloud image: Ubuntu 26.04.1, systemd
  259.5; `target/evidence/guest/probe-is-enabled-259.log`, `probe-masked-and-freeze-259.log`,
  `probe-kill-frozen-259.log`). `systemctl is-enabled`, word and exit code: never enabled
  `disabled` 1; enabled `enabled` 0, frozen or not; after `disable` `disabled` 1 (also of a
  frozen unit; `disable` again exits 0); enabled with `--runtime` `enabled-runtime` 0, and
  `systemctl disable` then exits 0 and leaves it `enabled-runtime`; a unit without `[Install]`
  `static` 0 (`disable` exits 0), `enabled` once a link made by hand wants it, and `static`
  again after `disable`, which removes that link; no such unit `not-found` 4; a vendor unit
  masked `masked` 1 (`disable` exits 0 and it is `enabled` again after `unmask`), masked with
  `--runtime` `masked-runtime` 1; `mask` of a unit whose file is in `/etc/systemd/system`
  fails ("already exists"). With the wants directory immutable (`chattr +i`) `systemctl
  disable` fails ("Access denied", exit 1) and the unit stays `enabled`. `yulius.service` as
  `host setup` leaves it: `enabled`, its file in `/etc/systemd/system`. `systemctl freeze
  yulius.service yulius-sandboxes.service` with the server's unit stopped: "Unit is not
  active", exit 1, and the sessions unit is frozen. The real `yulius.service` frozen
  (`Restart=on-failure`, `KillMode=process`): `systemctl stop` "Cannot perform operation on
  frozen unit", exit 1; `systemctl kill --kill-who=all --signal=SIGKILL` exit 0, then
  `ActiveState=activating`, `SubState=auto-restart`, `FreezerState=frozen`, `ControlGroup=`;
  `systemctl thaw` "Unit is not active", exit 1; `systemctl start` and `systemctl enable
  --now` "Cannot perform operation on frozen unit", exit 1. A restart of the guest cleared it.
  The real `yulius-sandboxes.service` frozen (no `Restart=`; in `scenario-i-*.log`): `stop`
  refused the same way; the kill exits 0, then `ActiveState=failed`, `FreezerState=frozen`;
  `thaw` "Unit is not active"; `start` refused; after `systemctl reset-failed` `inactive`,
  `FreezerState=running`, and `start` exits 0.
- **The real command on the guest** (`target/evidence/guest/scenario.sh`; logs
  `scenario-before-df0600a4b.log` for A to F with the build of before, and
  `scenario-fix-d5d364fdf.log` for A to H with this change; A to F also at `9cb8bd5d` and
  `2bbf653e`, before the review). `yulius host setup --public-url http://vm.test --no-smoke`
  (exit 0: the real `yulius.service` and `yulius-sandboxes.service`). The new host is the
  guest itself, reached by the command over ssh to `root@127.0.0.1`: one guest stands in for
  two, so everything the look asks is real (systemd, the cgroup files, `yulius instance
  migrate`, `--status`), and the "new instance" that answers the health question in A is the
  old one before it is frozen.
  - **A, the issue's own case**: a recorded start, the old units thawed and the unit enabled
    since, and a `systemctl disable` that fails for real (the immutable wants directory).
    Build of before (`df0600a4`): run 1 freezes both units again, "Failed to disable unit:
    Access denied", `FAIL old instance yulius.service must be frozen and disabled now …`,
    **exit 1**, state `Moved`, `finished`; run 2, nothing changed (`is-enabled=enabled`): `ok
    move finished: the instance runs on …`, **exit 0**, and `--status` shows that alone. This
    change (`d5d364fd`): run 1 the same up to the line, which reads

    ```
    FAIL     old instance           yulius.service is frozen, but `systemctl is-enabled yulius.service` says `enabled`. It must be frozen and disabled now that the new instance runs: `systemctl freeze yulius.service yulius-sandboxes.service; systemctl disable yulius.service`, then run the command again. The move is not finished until then
    ```

    **exit 1**, state `Moved`, `finished`, `--status`: "Move t461 to root@127.0.0.1: Cutover"
    with that line. Run 2: under `Cutover`, alone,

    ```
    FAIL     old instance           yulius.service is frozen, but `systemctl is-enabled yulius.service` says `enabled`. It must be frozen and disabled now that the new instance has taken over: `systemctl freeze yulius.service yulius-sandboxes.service; systemctl disable yulius.service`, then run the command again. The move is not finished until then. To go back to this host instead: `yulius instance migrate --abort`, once the new instance is stopped
    ```

    **exit 1**, `--status` "Cutover" with that line. Then `systemctl disable yulius.service`
    with the directory free again (exit 0). Run 3: `ok old instance yulius.service is frozen
    and disabled: it does not run beside the new one`, `ok move finished: …`, **exit 0**,
    `--status`: "done".
  - **B to F with the build of before**: exit 0 and "finished" in every run, with both units
    thawed (B), with the server's unit stopped and a process of it left, "populated 1 frozen
    0" (C), with the stopped unit enabled (D), with the sessions unit thawed beside the stopped
    server (E), and for a move not yet finished whose units were thawed (F: the finish is done
    and the run ends with exit 0).
  - **B to F with this change**: B, thawed: `FAIL … yulius.service is not frozen as a whole.
    It must be frozen and disabled now that the new instance has taken over: … If a freeze
    keeps failing …, stop it for good instead: `systemctl stop yulius.service
    yulius-sandboxes.service; systemctl kill --kill-who=all --signal=SIGKILL yulius.service
    yulius-sandboxes.service; systemctl disable yulius.service`. The move is not finished until
    then. To go back to this host instead: …`, exit 1; after `systemctl freeze` of both: `ok`,
    "finished", exit 0. C, stopped with a process left: `FAIL … does not run any more, but
    processes of it still run here beside the new instance … End them: `systemctl kill
    --kill-who=all --signal=SIGKILL yulius.service` …`, exit 1; after that command: `ok old
    instance yulius.service does not run here, nothing of it is left running, and it is
    disabled`, "finished", exit 0. D, enabled: `FAIL … does not run here any more, but
    `systemctl is-enabled yulius.service` says `enabled`: it may start again with this host
    …`, exit 1; after `disable`: exit 0. E, the sessions unit thawed: the line of B, exit 1;
    after its freeze: exit 0. F, moved and not finished, thawed: the line of B, then the
    finish's lines, exit 1, state `finished`; again: the line alone, exit 1; after the freeze:
    exit 0.
  - **G, a unit without `[Install]`** (the section removed from the real unit's file,
    `is-enabled=static`): `ok old instance …`, "finished", exit 0. With a link made by hand in
    `multi-user.target.wants` (`enabled`): the line of A's run 2, exit 1. After `systemctl
    disable` (exit 0, `static` again): exit 0.
  - **H, the way out taken**: the sessions unit thawed beside the frozen server: the line
    names `systemctl stop yulius-sandboxes.service; systemctl kill --kill-who=all
    --signal=SIGKILL yulius-sandboxes.service; systemctl disable yulius.service`, exit 1. Those
    three commands (exit 0 each), then the run: `ok`, "finished", exit 0. The frozen server's
    unit was not touched: `systemctl thaw yulius.service` exit 0, `active`, `running`.
  - **I, a resumed start with a stopped server and frozen sessions** (`scenario-i.sh`; logs
    `scenario-i-before-df0600a4b.log`, `scenario-i-fix-73b04f489.log`; the new instance is a
    stand-in on the guest: the same build, its own data directory, the old listeners). Build of
    before: `manual old instance yulius.service does not run here any more, and the new
    instance runs. Make sure nothing of the old one runs here: `systemctl stop yulius.service
    yulius-sandboxes.service; systemctl kill --kill-who=all --signal=SIGKILL yulius.service
    yulius-sandboxes.service; systemctl disable yulius.service``, exit 0. This change: the same
    line with `yulius.service` alone in the stop and the kill, exit 0, state `Moved`,
    `finished`; the same command again: `ok old instance yulius.service does not run here,
    nothing of it is left running, and it is disabled`, "finished", exit 0.
- **Gates.** `scripts/dev ci` at `73b04f48`, 21:41 to 21:46 UTC: exit 0; "4058 tests run: 4058
  passed, 16 skipped". Before the confirming round, at `d5d364fd`, 21:22 to 21:27 UTC: exit 0;
  "4058 tests run: 4058 passed, 16 skipped". Before the review, at `2bbf653e`, 20:26 to 20:34
  UTC: exit 0; "4057 tests run: 4057 passed, 16 skipped". `yulius security-scan`: exit 0, "Security scan
  disabled: Vidoc is turned off in Yulius, for this project or the whole instance; nothing was
  scanned." A disabled scan is not a passed one.
- **Independent review** (a fresh session given the diff range `3393b2ad..742779e7`, this spec
  and `om-code-review`; its own copy of the tree for builds, and the guest). Its words:
  "**approve.** There is no blocker and no major in the change's own code. There are 5 minor
  and 9 nit own-code findings to fix in this pull request, one pre-existing nit, and 2
  pre-existing findings with evidence." And: "Two limits on that verdict: I did not run
  `scripts/dev ci` (as instructed; your run at 2bbf653e8 is verified from its log), and the
  security scan is disabled, which is not a pass." It ran the 339 instance tests, `cargo fmt
  --check`, `cargo clippy -p yulius --all-targets`, `scripts/check_repository.py`, 18 mutants
  of its own (4 survived) and probes on the guest. Its findings, answered in `a7ec97de` (tests
  first), `2f7c4882` and `d5d364fd`:
  - minor 1, the fake systemd answered `is-enabled` for any unit: it answers by name now
    (`static` for the sessions unit, `not-found` for any other but the server's), and the two
    wrong-unit mutants are killed;
  - minor 2, `static`: taken as disabled (Decisions; flow test and guest G);
  - minor 3, `masked` cannot be reached through the command: out of the guide and the change
    log, the reason corrected (Decisions);
  - minor 4, an unanswered systemd in a later run: the line names no command (Decisions);
  - minor 5, "now that the new instance runs" in a later run: "has taken over", and the abort
    as the way back (Decisions);
  - nit 6, the way out without the disable: added;
  - nits 7 and 8, two sentences of the guide: reworded;
  - nit 9, a sentence of this spec about #1232: corrected;
  - nit 10, the comment and `linked`'s exit code in the unit test: corrected;
  - nit 11, the first line of the answer: a two-line case;
  - nit 12, the held look's heading: `Cutover` for both;
  - nit 13, two short lines in the architecture document: joined;
  - nit 14, no ADR-0310: Decisions (`record_number`);
  - 15, pre-existing, the weak reason in two test comments and SPEC-467: replaced;
  - P1, pre-existing, the way out naming frozen units: measured with the real unit and fixed in
    both lines (HLD-009, Decisions, guest H);
  - P2, pre-existing, a symlinked unit file and the cutover's disable: not this change;
    written in a comment on issue #1179.
- **The confirming round** (the same reviewer, the delta `742779e7..ef16e0d0`). Its words:
  "**approve.** There is no blocker and no major in the change's own code, and every
  first-round finding is answered correctly." And: "The delta brings 2 new minor findings and 3
  nits, all in or around the new HLD-009. They are fixed in this pull request, which moves the
  head again." It ran the 340 instance tests, `cargo fmt --check`, `cargo clippy -p yulius
  --all-targets`, `scripts/check_repository.py`, 6 mutants on the delta (4 survived) and
  probes on the guest, and checked this section against the logs ("All as written"). Its
  findings, answered in `fd57d9a4` (tests first), `ab9b0459` and `73b04f48`:
  - minor 1, half of HLD-009 unpinned: three flow cases (a later run with the server's unit
    thawed beside frozen sessions, with one on its way up, and with one stopped beside
    sessions that run; a resumed start whose two freezes fail), and its four mutants are
    killed;
  - minor 2, the "make sure" line named a frozen sessions unit: it names the stopped server's
    unit alone (HLD-009, Decisions, guest I), measured with the real sessions unit;
  - nit 3, "when systemd does not answer": the guide and ADR-0311 say "does not say what the
    units do";
  - nit 4, HLD-009 before HLD-008: in order;
  - nit 5, a line of 122 columns in the change log: the entry is wrapped again.
- **Not tested.** A move between two real hosts (the guest stands in for its own new host, so
  the steps on the new host after the cutover ran against the same machine); the real command
  on systemd 255 and 249 (this build runs on the 26.04 guest alone; what those versions answer
  to `is-enabled` was not measured); the page of an old host that reads the phase `Cutover`
  (`yulius-app::migration` is unchanged and reads any phase but `done` and `aborted` as not
  finished); the review's P2 with the real command (the reviewer measured systemd with a
  stand-in unit and read the move's side from the code); whether systemd 259 refuses to stop
  a unit whose freeze is still on its way (the way out names such a unit; neither the
  reviewer nor this task could produce the state).
