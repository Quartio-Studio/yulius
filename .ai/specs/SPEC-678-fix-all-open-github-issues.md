---
yulius-task: 29139e35-6467-465f-b585-6f80a004286b
status: in-progress
---

# SPEC-678: Fix all open GitHub issues

## Goal

The repository owner asked, on 2026-11-27, for every GitHub issue open that day (41) to be closed:
by a merged pull request that fixes it, or by a comment that says why it is already fixed or
cannot be fixed from this repository. Issues go in priority order (high, medium, low), one pull
request per issue or per small group of closely related issues, each test-first, independently
reviewed and merged into `dev` by `delivery_merge`.

## Context

- The 41 issues were read in full on 2026-11-27 (`gh issue list --state open --limit 1000`).
- Out of this repository's reach, by the owner's request: #1385 (quartio-v2's part of SPEC-497)
  is left as it is; #321 needs a measurement on the production VM's host, which a sandbox cannot
  reach (no `/sys/fs/cgroup`), so it gets a comment with what remains.
- Owner's decisions, asked with `needs_you` and answered on 2026-11-27: #638 keeps
  `GH_TOKEN`/`GITHUB_TOKEN` allowed (SPEC-004 GH-003 stays), so it was closed with that
  decision; #1239 removes the old fingerprint format now (the two weeks after 2026-11-13 have
  passed, and a session from before it would only stop running commands until continued).
- #1647's code fix merged in #1657; what remains is switching the App's webhook on, a person's
  action on the instance.
- Each slice below cites its issue; its own requirements and evidence are added to this spec in
  the pull request that delivers it. Non-goals: no new issues are opened (things noticed go into
  the final report), nothing is promoted to `main`.

## Requirements

- **FIX-001** Each issue's acceptance criteria are met by the slice that names it, or the issue
  is closed with a comment that names the fixing commit, or the reason it is not done.
- **FIX-002** A flaky test is fixed at its cause, shown by a probe at the racing step that fails
  before the fix; no retries, sleeps, longer timeouts or weaker assertions as the fix. A
  quarantined test leaves quarantine only once fixed (closing its issue ends the quarantine).
- **FIX-003** No slice widens a security boundary beyond what its issue asks.
- **FIX-1547** `task_message::a_flag_of_a_held_message_does_not_stop_the_pull_request_watch`
  passes when the news's submit hook reaches Yulius after its turn's Stop (the race behind its
  "timed out waiting for the turn's end" failures).
- **WAIT-001** (#1727, #1831) `WaitState` carries the task's latest staging request for its
  current merged revision (`staging_request`, `staging_state`, `staging_outcome`); a request of
  an older revision, or a task whose next pull request is open, has none. Owner: the server's
  `agent_wait_state`, read on every long poll.
- **WAIT-002** While that request is pending (testing, inherited, ready, deploying) the merged
  head's ended checks, its replayed merge notice and its staging deploy's CD run
  (`staging_cd_run`: the request's `cd_run_id`, or during a retry the task's latest run whose note
  names the request) do not end the wait; any other CD run of the task (another environment, a
  rollback) is reported when it ends, as before. `limit_line` treats the pending request as
  under way (no time limit; the next span still bounds an outage of Yulius). The wait ends when the request leaves pending, with `staging_outcome_text`
  (id, revision, state, runs, next step).
- **WAIT-003** A wait that starts at the merged head (its replayed merge notice, its ended checks
  or its staging deploy's unreported end) when the request already has its outcome ends with that
  outcome instead of the merge notice that said there was no request; with no request the merge
  notice and its hint stay as they are. The outcome is redacted for the sandbox, as its typed
  notice is.
- **WAIT-005** The server compares `since` only on the fields the client sent, so a `yulius wait`
  older than the staging fields (one that survives an update) is not answered at once in a loop.
- **WAIT-004** The `cd_deploy` receipt of a pending request says that `yulius wait` with
  `run_in_background` ends with the outcome.
- **AUT-1824** A scheduled run, the start's catch-up run, a manual run or a run a crash left
  `scheduled` (resolved at the next start) of an automation whose creator is disabled (or no
  longer a user) creates no task; the run is recorded as
  `skipped_disabled` with the reason "its creator, <login>, is disabled" (the automation's page
  shows a run's reason). The check reads the creator at each slot, so enabling the person again
  lets the automation run at its next time; nothing about the automation itself changes.
- **PROC-1795** A watch run's write past its file bound fails with EFBIG ("File too large") on
  every host: the wrapper ignores SIGXFSZ for the run's processes, so the disposition the server
  inherited (Python ignores it; a plain shell does not) no longer decides how the bound shows.
  On a production host, where the server starts sandbox shells with SIGXFSZ at its default, this
  changes what a watch script meets: a write past the bound is refused instead of killing the
  writer (exit 1, not 153), a shell loop writing to a full file now runs to its CPU bound, and a
  script's own `trap … XFSZ` has no effect. A run that exits or is killed at the bound still
  ends on its output bound and says so; one that keeps looping ends at its CPU bound or timeout
  with that reason.
- **PROC-1775** The MCP probe test counts a zombie grandchild as gone (`/proc/<pid>/stat` state
  `Z` on Linux), so it passes whichever process reaps it, and still fails for one still running.
- **PROC-1826** The sweep releases Cargo's locks with `LOCK_UN` before closing them (`Lease`),
  also when it is refused part way: `flock` belongs to the open file, and a process forked
  meanwhile keeps a duplicate until it execs, so a plain close left the lock held and the next
  sweep (or Cargo) read busy. Tests release their own locks the same way, and the compile
  cache's prune releases its lease the same way (`disk.rs`, the same cause).
- **PROC-1760** The identity tests write every executable they run (`write_exe`) from a child
  process, so no descriptor of the test process can be inherited by a child another thread
  forks, which made running the script fail with ETXTBSY ("Text file busy").
- **WAIT-1807** The compile-cache shell tests' wait for the shell's reports
  (`wait_for_report`) spends its 5 s only while nothing of the shell's session is runnable (R or
  D): a host that has not scheduled the shell or its job yet spends none of it, while a stopped,
  dead or sleeping shell does, so a shell that really fails to report still fails within the
  budget. A wall-clock ceiling of 60 s ends the wait whatever runs (a spinning shell or job).
- **WAIT-1820** The Telegram reply test waits for the second run's own notice before it
  records the message count, so the next message after its reply can only be the answer.
- **DOC-1769** AGENTS.md, `docs/engineering/specifications.md`, the bundled
  `delivery-pipeline` skill and the scaffolded AGENTS.md say what `spec_required` does: the
  workflow's `spec:` gate decides (Feature, Fix and Infra bootstrap require a spec; Chore, Docs
  and Research / Question do not), and a task without a workflow needs one once its branch
  changes more than documentation; a chore that changes code runs as the Chore workflow. The mechanism is unchanged (a documentation fix: the
  owner's cleanup charter's chores start with the Chore workflow, or write a short spec).
- **DOC-1761** The infra-bootstrap skill says production-stage `[cd.*]` secrets run outside
  agents' reach and dev-stage ones need the allowance that lets any task copy them (ADR-0105).
- **BRF-1800** The briefing's mandatory parts fit `DEFAULT_BUDGET_BYTES` whole in every changes
  layout (log newest first, log newest last, a folder of notes), for every spec policy and
  harness, with the security scan's section: the scan section, the folder's change-note rule, the
  "Read before you build" changes line and two sentences are shorter, and an instruction file's
  path is cut at 60 bytes (was 120); no rule is dropped ("rerun it" for the scan's exit 3 goes:
  the scanner says "run it again" itself). Out of scope, for the final report: a task whose
  workflow instructions lower the budget below the full 9,500 can still be cut (the review of
  #1838 measured it); that is how the budget is split, not #1800's overflow.

## Plan

- [x] #1547: the held-message test's cause shown fixed on dev by #1732; its quarantine ended
- [ ] #1831 #1727: yulius wait follows the task's staging request
- [ ] #1824: a disabled person's automations create no task
- [ ] #1808: the parking candidate's later stall is reported deterministically
- [ ] #1809: the cut-off reviewer's Opus note test waits on the note's trigger
- [ ] #1807: the compile-cache Ctrl+Z test waits on the stopped job
- [ ] #1769 #1761: chore tasks and the documents agree on specs; skill wording on dev-stage secrets
- [ ] #1765: privileged containers on Ubuntu 26.04 start or are skipped with the reason
- [ ] #1133 #321 #1647: remaining queue analysis and host-only checks recorded on their issues
- [ ] #1826 #1760: sweep and identity shim lib tests are deterministic
- [ ] #1820: the Telegram reply test waits for the second run's notice
- [ ] #1817: a test holds prove-on-dev's refusal when dev cannot be deployed
- [ ] #1811: the security scan card reads vidoc.toml as the scanner does
- [ ] #1800: the briefing's mandatory parts fit the budget in every docs layout
- [ ] #1795: a watch's write past its file bound is refused (EFBIG) on every host
- [ ] #1775: the MCP probe test counts a zombie grandchild as gone
- [ ] #1767: the memory stopgap walk counts every entry and reads folders by descriptor
- [ ] #1758 #1714: dialogs give the keyboard back to the redrawn opener or the failed action
- [ ] #1753 #1751: non-owner creators in a personal account or a narrowed App are told who acts
- [ ] #1741 #1604: opt-in real-Chrome tests wait for input and accept the decided results
- [ ] #1702: Codex goal, intake and context-full input repaint an unreadable screen
- [ ] #1693: the Antigravity error-turn hook test is deterministic
- [ ] #1692: a replaced run's reattach leaves the task's writer alone
- [ ] #1691: a decided armed merge is consumed across a restart
- [ ] #1688: a cancelled display start keeps its Xvfb in the slot
- [ ] #1595: confined test fixtures share one temporary root
- [ ] #1591: parked-list and CI watcher fixtures hold under shared CPUs
- [ ] #1544: egress warning-capture tests observe their own warning
- [ ] #1532: a branch fast-forwarded to a head with skipped jobs gets its own run
- [ ] #1520: the Superagent deep-link panel test is deterministic
- [ ] #1239: the old fingerprint format is removed and a version 1 file is rewritten as version 2
- [ ] Final report of every issue's outcome given

## Acceptance criteria

- **AC-1** (FIX-001) Every issue open on 2026-11-27 is closed by a merged pull request or a
  justified comment, or is named in the final report under "Not done" with its reason.
- **AC-2** (FIX-002) Each flaky-test slice records the probe that reproduced the failure and the
  same probe passing after the fix.
- **AC-3** (FIX-1547) With a probe at the racing step the test fails without the fix and passes
  with it, and its other assertions are unchanged.

- **AC-4** (WAIT-001..003) A merged task with a real staging request in `testing`: the wait
  state names it, `settled` returns nothing over the head's passed checks and `limit_line` none;
  after the queue verifies it, `settled` returns the request's verified outcome (with CI and CD
  run ids) and a wait started afterwards gets the same line; a later landing has no request.
- **AC-5** (WAIT-002, WAIT-003) Without a staging request the merge notice still ends the wait;
  another CD run of a task with a current request (decided or pending) is reported when it ends.
- **AC-6** (WAIT-004, WAIT-005) The pending receipt names `yulius wait`; a `since` without the
  staging fields does not differ from the state on them.

- **AC-7** (AUT-1824) A creator disabled through `PATCH /api/admin/users/{id}`: the next
  scheduled slot, a run left `scheduled` and the start's catch-up create no task, and each run
  says why; enabled again, nothing missed is replayed and the next day's slot creates one;
  Run now with the creator's context from before the disable is skipped the same way.

## Test plan

Each slice adds or changes the test its issue names, run in full with its test binary; flaky
fixes are reproduced with probe delays at the await point (not by load) and stressed with
`nextest --stress-count` beside busy loops. Yulius CI runs the whole gate on each head.

## Decisions

- One spec for the whole task, one pull request per issue or group, delivered in turn from this
  branch: the task has one branch, and `delivery_merge` delivers the finished part of the plan
  (ADR-0325). Each slice's details are added in its own pull request.
- Issues needing a person's action or decision are asked with `needs_you` and the work goes on
  with the others, as the owner's request says.
- #1824: a disabled creator's runs are skipped (`skipped_disabled` with the reason), rather than
  disabling the automation when the person is disabled. Skipping needs no state to restore when
  the person is enabled again and cannot lose an automation someone else disabled on purpose; a
  new run state would change the wire contract and the pages for the same information the
  reason already carries (migration 0072 records a paused schedule the same way).

## Evidence

### #1547 (closed, fixed on dev by #1732)

- All three failures (PR #1536, #1543 on 2026-11-20; task #596 on 2026-11-22) were `Error:
  "timed out waiting for the turn's end"`. #1732 (16556d990, 2026-11-23) changed `hook_event` to
  read a hook's send time on Yulius's clock (`Clock::of_wall`).
- Probe at `race::Point::HookArrived` (`<session>/UserPromptSubmit`), holding the news's submit
  until the news turn's Stop is answered: on dev d6e17997d the hold fired and the test passed
  (6.5 s). With `hooks.rs:251` reverted to `wall.0.min(now)` it failed as in CI,
  `activity=Some(Working)`, "timed out waiting for the turn's end" (36.5 s).
- Unchanged test, 140 busy loops, `nextest --stress-count 40 --test-threads 40`: 40/40 passed.
- Closed on GitHub with that evidence; the quarantine ends with the issue.

### #1831, #1727 (yulius wait and the staging request)

- New: `wait_cli::tests::a_pending_staging_request_keeps_the_wait_until_its_outcome`,
  `a_staging_deploy_ends_the_wait_with_the_requests_verdict`,
  `another_cd_run_of_a_staging_task_is_still_reported`,
  `each_path_to_a_staging_outcome_ends_the_wait`,
  `without_a_staging_request_the_merge_notice_stays`,
  `agent_wait::tests::a_client_is_compared_only_on_the_fields_it_sent`, and the integration test
  `staging::yulius_wait_follows_a_merged_tasks_staging_request` (real Git, queue, deploy and
  verify scripts; receipt sentence, CI and CD run ids, open next pull request, later landing).
- Mutation: without `!staging_pending` on the checks-ended line, both pending tests fail.
- `staging::a_staging_deploy_is_the_requesting_tasks_cd_run` (SPEC-631 SWK-004) still sees the
  staging deploy as the task's CD run in the wait state.
- Review round 1 (Fable 5.2, request changes at 7ce41f021): F1 major (another CD run swallowed or
  misreported) fixed with `staging_cd_run`; F2 (older client answered at once) fixed with
  `differs_from`; F3 to F6 and the nits (tests per branch, receipt and CD id assertions,
  ADR-0466 refined, WAIT-003 scoped to the merged head, redaction) answered.
- Review round 2 approved 6cb380e57. Its minor (a retry clears the request's run before the next
  is recorded, so the earlier attempt's end could end a wait started then) is fixed test-first:
  `staging_cd_run` falls back to the task's latest run whose note names the request
  (`ci::staging::deploys_request`); the integration test's retry step failed before the change.

### #1824 (a disabled person's automations)

- `automations::a_disabled_creators_automation_creates_no_task` failed before the change (a task
  was created at the first slot) and passes after it; its crash-left `scheduled` step failed
  until `resolve_pending` checked the creator too; `automation` tests: 26 passed.
- Review (Fable 5.2, after a first reviewer was cut off by a usage limit) approved ef893618c.
  Its minors answered: `resolve_pending` is the single check (the `process_slot` branch was
  redundant: every path ends there, mutation M1 survived), and
  `automations::a_disabled_creator_gets_no_task_by_run_now` covers Run now with a stale context.
  Not covered by a test: the "no longer a user" arm (no production path deletes a user).

### #1795, #1775, #1826, #1760 (inherited process state in tests)

- #1795: run directly from a shell (SIGXFSZ at its default; `scripts/cargo` is Python, which
  ignores it), `watches::run_bounds_are_enforced` failed; with the wrapper's `trap '' XFSZ` it
  passes. Unit test `ci::watch_run::tests::a_write_past_the_file_bound_fails_with_efbig_whatever_the_disposition`
  (python3 starts the wrapper with SIGXFSZ default) failed with `head exited 153`, "File size
  limit exceeded", before the change.
- #1775: under a Python subreaper that never reaps orphans (`PR_SET_CHILD_SUBREAPER`, as the
  scratch-target wrapper), `silent_stdio_server_times_out_and_is_reaped` failed "grandchild …
  survived"; with the zombie check it passes there and normally.
- #1826: `sweep::tests::a_duplicate_of_a_sweeps_lock_does_not_outlive_the_sweep` (a `try_clone`
  stands for a forked child's duplicate) failed "the sweep's lock outlived it in a duplicate"
  before `HeldLocks` unlocked on drop.
- #1760: holding a write descriptor of the fake executable made the shim test fail with
  `exec: …: Text file busy` (126); the test executables are now written by a child process. The
  issue's 50 runs under load: the deterministic probes above stand for them; the review of
  #1836 ran the whole `yulius-app` lib binary 300 times (200 unpinned, 100 on 4 CPUs with 64
  threads) with no failure in the sweep, identity, probe or watch tests.
- Review round 1 of #1836 (Fable 5.2, request changes at 638a97f): the macOS dead-code blocker
  was fixed at 0755a5d; its major (locks taken before a refusal were only closed) is fixed with
  `Lease` and `lock_profiles_into`, tested by
  `sweep::tests::a_sweep_refused_part_way_releases_the_locks_it_took` (it and the duplicate test
  fail when `Lease` does not unlock); the wrapper test runs with `LC_ALL=C`; the other identity
  test executables go through `write_exe`; `disk.rs`'s lease uses `Lease` too.

### #1807, #1820 (tests wait on the condition)

- #1807: probe that leaves the shell runnable but unscheduled for 8 s right after Ctrl+Z
  (`SCHED_IDLE`, pinned to CPU 0 beside 80 busy loops, as on a loaded host): dev 4be78a780 fails
  `shell did not report b'Stopped': b'^Z'` as in CI; the fix passes (8.17 s). New test
  `test_a_shell_that_cannot_report_fails_the_wait_within_its_budget`: a stopped shell fails the
  wait within its budget. `python3 -m unittest discover -s scripts -p test_compile_cache.py`: 50
  OK.
- Review round 1 of #1837 (Fable 5.2, request changes at a0d6759): its major (the budget never
  accrued for a stopped, dead or spinning shell, so the wait could hang) is fixed by counting only
  runnable states, reading the whole session, and the 60 s ceiling; `/proc` is read as bytes;
  the clock mark moves after each read; the Telegram wait reads the count before its look.
- Review round 2 (request changes at 3bf1ad6): the regression test raced the fork and asserted
  wall-clock time; now the child says it is ready, the test waits until it is stopped (`T`), and
  asserts the wait ended on its idle budget (`wait_for_report` returns which bound ended it).
  With a stopped shell counted as runnable it fails (`'ceiling' != 'idle'`). The ceiling is
  checked on every pass, output or not.
- #1820: probe delaying the Needs-you notice 300 ms and the reply's answer 1 s reproduced the
  CI failure exactly (`left: "Needs your decision · app …"`, line 3000); with the test waiting for
  the second run's notice first it passes with the same probe.

### #1769, #1761, #1800 (what agents are told)

- #1769: checked against the code: `SpecPolicy::from_workflow` maps the Chore, Docs and Research
  workflows' `spec: optional` to not required, Feature and Fix to required, no workflow to
  `ByChange`; the documents now say so. `scripts/check_repository.py` passes.
- #1761: the skill's sentence matches `allowed_for` (`vault/env_files.rs`): dev stage needs
  `EnvFiles::Allowed`, production stage `cd_prod` or `auto_allow`.
- #1800: new `briefing::tests::the_worst_case_with_the_scan_keeps_every_mandatory_part_whole`
  failed on dev ("the scan section was cut"; mandatory 9,530 bytes, 9,723 in the folder layout,
  against 9,500). After the rewording the test covered Required/Claude Code only; review round 1
  (request changes at e3398c3) showed Codex and `ByChange` still cut the scan section. The test
  now runs 27 cases (3 layouts × 3 policies × 3 harnesses); with the old 120-byte cap on
  instruction-file paths it fails ("log, newest first, Required, Codex: the scan section was
  cut"); with 60 it passes.
