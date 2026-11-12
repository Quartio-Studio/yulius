---
yulius-task: fedcfccc-a17b-4ef0-b435-c6fa39b5e17e
status: in-progress
---

# SPEC-481: A move is not finished while its old instance is not held (issue #1179)

Status: in-progress
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
- **Sibling**: issue #1178 (SPEC-478, task #460), whose branch this one starts from. It added
  `freeze::left_running` (what a unit that does not run left in its cgroup), which the look here
  uses. Its own failed case records nothing and is left as it is.
- **Asked along by the issue and its comments**: the line of `start()` for a new host that
  stopped answering names `--abort` without saying when it is safe; `finish()` takes a key the
  new host refuses for "done there" only when the new URL answers, and no test holds the second
  half; a comment gives a weak reason for not trying a step over a missing key.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| HLD-001 | **Held.** The old instance of a move whose new instance has taken over is held when systemd says all of this: no unit of it runs unfrozen (the server's unit is frozen and its sessions unit, where it runs, is frozen; or the server's unit has stopped, `inactive` or `failed`, and its sessions unit does not run unfrozen); no unit of it that does not run has left processes that run (`freeze::left_running`); and `systemctl is-enabled` of the server's unit says `disabled`, `masked` or `masked-runtime`. An unanswered question is not held. | unit tests of `freeze::disabled`; the flow tests below |
| HLD-002 | **Every later run looks.** A run that finds the move recorded as moved, finished or not, asks systemd as in HLD-001 before anything else of the move, and changes nothing of the old instance: it freezes, thaws, stops, starts, enables and disables nothing. Held: an `ok` line says how (frozen and disabled, or does not run and is disabled). Not held: a `FAIL` line names the commands, as after the start, and says that the move is not finished until then; the run ends with exit 1, the phase of `status.json` is `Cutover`, and no "finished" line is printed. | flow tests `a_move_whose_old_unit_was_not_held_is_not_finished_until_it_is`, `a_finished_move_whose_old_instance_no_longer_holds_fails_until_it_does` |
| HLD-003 | **The finish is still done.** A run that finds the move moved and not finished goes on to the steps after the cutover and to the finish whether or not the old instance is held, as the run that started the new instance does. `finished` in the state file keeps its meaning (the finish was done: the key is gone); "finished" is said, with exit 0, only by a run that finds the finish done and the old instance held. | the same tests; `a_finish_that_did_not_reach_the_new_host_is_tried_again` |
| HLD-004 | **The look after a start is the same look.** `moved()` sends `systemctl disable` as before and then decides by HLD-001: the failed line when the instance is not held, also when a unit of it that does not run left processes. A server's unit that has stopped with nothing left is said as that, not as "stays frozen". | flow tests; the existing tests of the failed line |
| HLD-005 | **A stopped unit must be disabled too.** A resumed start that finds the old server's unit stopped with nothing of it left and the new instance running keeps its `manual` line and records the move; when the unit is not known to be disabled a `FAIL` line names `systemctl disable <unit>`, the run ends with exit 1, and later runs look (HLD-002). | flow test `a_recorded_start_with_a_stopped_server_that_is_not_disabled_fails_until_it_is` |
| HLD-006 | **`--abort` is named with its condition.** The line of `start()` for a new host that stopped answering says "`yulius instance migrate --abort` only once the new instance is known to be stopped", as the lines of SPEC-467 LFT-003 do. | flow test `a_lost_answer_to_the_start_is_decided_by_asking_the_new_host` |
| HLD-007 | **A refused key with a silent URL is not "done there".** Held by a test (the behaviour is unchanged): the key stays on the old host, nothing is recorded as finished, and the run says to run the command again. | flow test `a_refused_key_with_a_silent_url_is_not_a_finish_found_done` |
| HLD-008 | `docs/operations.md` §7 ("Move the instance to another host") says what a later run does; `docs/changes.md` has the entry; ADR-0311 records what "finished" means. | the documents |

## Plan

- [ ] The defect reproduced: flow tests that run the command again after the failed line fail on the unfixed code
- [ ] One look decides whether the old instance is held, after a start and in every later run of a moved or finished move
- [ ] A stopped unit that is not disabled fails, the abort line names its condition, and the finish's refused-key case is pinned
- [ ] The guide, the change log and ADR-0311 say what a later run does and what finished means
- [ ] The same runs on a real host: an Ubuntu guest with the build before and after the fix
- [ ] Every gate run and the independent review's findings answered

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
- **Mutations** (never committed): the look removed from the finished branch; from the moved
  branch; the disabled question dropped; the leftover question dropped; "enabled" taken for
  disabled; an unanswered systemd taken for held; the later look made to disable or freeze; the
  finish skipped when the look failed. Each must fail a test.
- **A real host**: an Ubuntu cloud image as a QEMU guest. What `systemctl is-enabled` answers
  in each state; then the real `yulius instance migrate` on a finished move whose old unit was
  thawed or enabled, with the build before and with this change, and again once it is held.
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
  `systemctl disable` exiting 0 says that a command ran, not what holds now. `disabled`,
  `masked` and `masked-runtime` are the answers that mean "does not start with the host";
  every other answer, an empty one and an error are "not known to be disabled", and the line
  quotes what systemd said. `moved()` still sends `disable` first and then decides by the same
  look, so one rule holds after a start and in every later run.
- **The steps after the cutover do not wait for the old instance.** They are the new
  instance's (its firewall, its key, its tasks), and the look needs no key. So a run that fails
  the look still finishes, as the run that started the new instance always did.
- **The phase of a failed later look is `Cutover`.** The page offers the abort while the phase
  is not `done` (`yulius-app::migration`), which is right for a move that is not finished; and
  the old instance held is the cutover's own promise.
- **SPEC-478's failed case is left as it is** (nothing recorded, the move stays `Starting`): it
  is safe either way, its pull request is not merged yet, and changing it is not this issue.
