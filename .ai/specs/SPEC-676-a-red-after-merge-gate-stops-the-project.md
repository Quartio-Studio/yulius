---
yulius-task: 47d78142-00f1-4f97-aef2-662e7e27c28b
status: in-progress
---

# SPEC-676: A red after-merge gate stops the project's merge queue until the integration branch is green again

## Goal

Under merge-first staging, the full suite runs after a pull request merges. While that
after-merge gate is red on the integration branch, Yulius merges nothing more into it, except
the repair. Merges resume by themselves once a later verdict is green. Everyone waiting is told
why their merge waits. The task whose merge broke the gate is told to repair it first.

## Context

The owner decided this on 2026-11-26 in the quartio-v2 project. They keep merge-first staging
(`[cd.dev] merge_first = true`, `after_merge` test shards) because running the full suite before
every merge takes too long. They added one rule: a red after-merge gate stops the merge queue.

What happened without it, on quartio-v2 on 2026-11-25:

- Pull requests kept merging onto a `dev` whose after-merge gate was red. Yulius then attributed
  later failures to the wrong task: task #414 got "inherited failures from #412", and task #415, a
  docs-only pull request, was shown "tests (4/4) failed".
- Tasks repaired `dev` for one another (AGENTS.md: "a red dev is repaired by the tasks still at
  work"), and staging stood still.

What exists:

- **The merge queue** (`crates/yulius-app/src/agent_wait.rs` `merge_armed_pass`,
  `merge_armed_task`; `crates/yulius-app/src/merge_train.rs`, ADR-0285).
  - One pass runs per sandbox at a time.
  - An armed pull request merges once its own run passed and it is up to date. One that is
    behind joins a merge candidate.
  - `delivery_merge` merges at once when the head is green and current, and arms otherwise.
- **The gate history** (`crates/yulius-app/src/ci/gate_history.rs`, SPEC-612, SPEC-653,
  ADR-0450).
  - It replays the integration branch's settled full-gate verdicts since its last full pass, in
    first-parent order.
  - Each open failure is named with the revision that first showed it and that revision's merge
    owner. `integration_failures` gives them as `GateNotice`s.
  - `tell_gate_repairs` assigns one available worker the repair and tells the others who owns
    it. The CI page shows the failures to people when no worker is available.
- **The flaky quarantine** (SPEC-497 OFC-002, ADR-0356).
  - It keeps `ci_quarantine` rows by test name and tells every job `YULIUS_CI_QUARANTINE`.
  - quartio-v2's ADR 0271 leaves those tests out of its gate.
- **`[merge_queue]`** of `yulius.toml` already holds `apart` (SPEC-497 MQW-003), read at the
  integration head.

## Requirements

- **RGS-001 The setting.**
  - `yulius.toml` `[merge_queue] stop_on_red_gate = true | false`, read at the integration head.
  - Without the key it is on exactly when the project has a merge-first staging target
    (`[cd.<name>] merge_first = true`), and off otherwise. An unreadable or invalid file counts as
    off.
- **RGS-002 When the queue is stopped.**
  - The setting is on, and the gate history at the integration head has an open failure that is
    not a quarantined test of the project.
  - An open failure is one shown by the newest settled verdict, or a failure since the last
    full pass that no later verdict has cleared.
  - A settled red verdict whose range is still being narrowed (SPEC-653), or that is newer than
    an active verdict, stops the queue at once, unattributed: only a marked repair passes.
  - A failure kept open only because later passing runs skipped its job stops nothing; it stays
    a notice for people.
  - The state is read at most every 15 s per project. A new integration head is read at once.
- **RGS-003 Waiting merges stay armed.**
  - While the queue is stopped, the armed-merge pass merges no pull request that is not a
    repair. Each such merge stays armed and keeps its place.
  - Its queue note (typed into its session by the pull request watch, shown by `yulius wait` and
    the task page) says three things: the merge waits because the after-merge gate of the
    integration branch is red; which failures, from which revisions and tasks; and that it merges
    by itself once a later revision's gate is green.
- **RGS-004 `delivery_merge` on green while the queue is stopped** arms the merge instead of
  merging. A repair is the exception. Its answer says why, in the words of RGS-003.
- **RGS-005 A repair merges.**
  - A pull request is the repair when its task's agent calls `delivery_merge` with
    `repairs_gate: true` (kept with the armed merge and audited), or when Yulius names its task
    as the merge owner of an open failure.
  - A repair merges as any merge does, even while the queue is stopped.
- **RGS-006 Merge trains.** While the queue is stopped:
  - a merge candidate forms only from repairs;
  - a candidate under test that holds a pull request that is not a repair is dissolved, with
    why `gate_red`. Its members stay armed.
- **RGS-007 It resumes by itself.** Once no open failure is left (a later revision's gate, or a
  rerun of the red one, passed), the next pass merges as before, without anyone doing anything.
- **RGS-008 Quarantine.** A failure that is a quarantined test of the project never stops the
  queue.
- **RGS-009 The breaker repairs first.**
  - When the gate repair is assigned, the task whose merge Yulius names as the origin of an open
    failure is preferred among the available workers. The worker who already owns the repair
    keeps it.
  - While the queue is stopped, the assignment's message also says that merges wait for the
    repair, and that the repair's `delivery_merge` passes `repairs_gate: true`.
- **RGS-010 The CI/CD page shows it.**
  - `GET /api/sandboxes/{sid}/ci` has `merge_stop` while the queue is stopped: the failures, the
    run to read, and the pull requests whose merges wait.
  - The CI tab shows it as a banner above the runs.

## Plan

- [x] Spec and ADR: the stop, its setting and default, repairs, quarantine, trains
- [x] The decision: `[merge_queue] stop_on_red_gate`, the stop from open failures without quarantined tests, repairs and the waiting note
- [x] The armed-merge pass and `delivery_merge` hold merges that are not repairs while the queue is stopped; `repairs_gate` marks a repair
- [x] Merge trains form only from repairs while the queue is stopped; a candidate under test is dissolved
- [x] The breaker is preferred as the repair owner and told the queue waits for the repair
- [ ] Docs: architecture, product, API, changes; independent review of the backend
- [ ] Deliver the backend: green CI and merge into dev
- [ ] The CI/CD page shows the stopped queue (API `merge_stop`, banner, mock, e2e)
- [ ] Independent review of the page, green CI and merge into dev
- [ ] Staging deploy of the merged revision and the closing report in Polish

## Acceptance criteria

- On a merge-first project, a red after-merge verdict at the integration head keeps a green,
  up-to-date armed pull request from merging. It stays armed with a note naming the failure.
- A pull request armed with `repairs_gate: true`, or one from the task that broke the gate,
  merges while the queue is stopped.
- After a later revision's verdict passes, the waiting pull request merges in the next pass.
- When the only failure is a quarantined test, the queue does not stop.
- A project without a merge-first target, or with `stop_on_red_gate = false`, merges as before.
- The CI tab of the CI/CD page shows the stopped queue with its failures.

## Test plan

- Domain unit tests: the setting's parse and default; the stop from failures with and without
  quarantined tests; repairs by mark and by attribution; the waiting note's words.
- Integration tests in `crates/yulius/tests/merge_train.rs` (real repositories, fake GitHub):
  - red stops the queue;
  - a marked repair merges and a breaker's pull request merges;
  - green resumes;
  - a quarantined failure does not stop it;
  - off without merge-first and with `stop_on_red_gate = false`;
  - a candidate under test is dissolved and forms only from repairs.
- Store test: the breaker is preferred as the repair owner, and the incumbent keeps it.
- API test of `merge_stop`; an e2e spec of the banner on the CI tab.

## Decisions

- The setting lives in `[merge_queue]` of `yulius.toml`, beside `apart`, not in the database. A
  repository's own file says how its changes merge, a pull request changes it, and the instance
  already reads that table at the integration head.
- It is on by default where merge-first staging is on, because that is the only delivery where
  merges skip the full suite. Without it, every merge-first project repeats quartio-v2's
  2026-11-25. Without merge-first, the full suite gates each pull request, and the integration
  branch's verdicts change nothing. That is why the default is off there. A project that prefers
  to keep merging sets `stop_on_red_gate = false`.
- The stop is computed from the existing gate history (`integration_failures`), not a new
  record. That keeps one meaning of "open failure" for attribution, the repair assignment and the
  stop.
- A repair is the agent's mark or Yulius's attribution, and both are audited. A wrong mark merges
  one pull request onto a red branch, which is today's behavior, so the mark needs no person.
- A person's own Merge on the task page while the checks passed is not held. It is a person's
  decision. Their armed merge waits like any other.
- An errored integration run counts as red, as attribution counts it. A rerun that passes clears
  it.
- Recorded in [ADR-0482](../../docs/decisions/0482-a-red-after-merge-gate-stops-the-merge-queue.md).
