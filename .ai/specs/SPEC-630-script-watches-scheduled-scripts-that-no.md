---
yulius-task: d09a439b-401f-4a27-8a5f-78b535f91ab9
status: in-progress
---

# SPEC-630: Script watches: scheduled scripts that notify a person without an agent turn

## Goal

A person, or their Superagent, asks Yulius to watch something: a URL, a release, a CI number, a
date. Today every such check is an automation or a Superagent check, and each one spends an agent
turn even when nothing changed. For example, the owner's daily check "#479 merge-queue targets"
wakes the Superagent every day only to answer "not due" until 2026-11-24.

A **watch** is a shell script plus a schedule, owned by a person in a project. Yulius runs it in
the project's sandbox, with no agent and no subscription. Whatever the script prints becomes a
notice for its owner, keyed `watch:<name>`, so each report replaces the one before it. The owner
approved this on 22 Nov 2026 after a review of iTermX, a separate tool. Its design and runner
(`itermx-watch`) were read as a reference, and none of their code is used.

## Context

- **Notices** (SPEC-624 part 1, ADR-0462, merged in #1705): `App::post_notice(NewNotice)` stores
  a notice under (project, recipient, key) and replaces an earlier one with the same scope (newest
  wins). It resolves an expiry (`never`, `30m`, `2h`, `3d`, RFC 3339) and publishes
  `notice.posted` to the people it reaches. Their Telegram chats get it through the notifier,
  under their channel choices (`NoticeKind`). `watch:` is reserved for this task, and the
  spec's table says a watch's output gets "the watch's expiry, else 1 day". The bell gets server
  notices in SPEC-624's second pull request, which is still open. Until it merges, a watch's
  notice reaches `GET /api/me/notices`, `notices_list`, the digest and Telegram, and its last
  output shows on the Watches page.
- **Automations** (`automations.rs`, SPEC-001 AUT-*) create a *task* on a cron schedule
  (`yulius_domain::cron::Schedule`, with a UTC offset). Their scheduler ticks every 30 s.
- **Superagent checks** (`superagent_checks.rs`, SPEC-487 CHK-*) type a prompt into a person's
  Superagent every 5 minutes to 1 day, and are kept in `superagent-checks.json`.
- **Sandboxed runs without an agent** already exist: CI jobs (`ci/exec.rs`) and delivery
  environment runs (`ci/environment.rs`). Each is a `LaunchPlan` started through the process host
  (`ci_watch`: time limit, cancellation, kill grace, output ring), in the sandbox's containment
  (bubblewrap on the sandbox root, joined to the sandbox's namespace holder in CI's cgroup,
  `ci_join`). A CI job gets agent secrets as placeholders, plus an egress proxy credential that
  only covers the secrets its grant names (`ci_egress_open`, `EgressCaller::Ci`, SPEC-142).
- iTermX's rules, as the owner described them: a watch is a script plus a schedule, run by a
  small daemon, and outlives the chats. Exit 0 with output posts it, exit 0 with no output posts
  nothing, exit 3 posts and stops the watch, and any other exit is a failure. Three failures in a
  row post one warning. Posts that cannot be delivered are kept and retried. Each post is keyed
  `watch:<name>`, so the newest replaces the previous. A Jobs page offers Run now, Pause/Resume
  and Remove.

Non-goals: watches outside a project (instance-wide); iTermX's `--changes`, `--once`, `--ask`
and `--keep` (a script can compare against a file in its state directory, and exit 3 covers
"once"); running a watch as a person's identity (Git as their account, their personal secrets);
posting to people other than the owner.

## Requirements

- **WCH-001 A watch.** A watch belongs to a project and has:
  - a **name**: 1 to 40 characters, lowercase letters, digits and `-`, starting with a letter or
    digit, unique in the project;
  - a **script**: 1 byte to 16 KiB of bash;
  - a **schedule**: either an interval (`every`, 1 minute to 7 days) or a cron expression with a
    UTC offset, as automations take them;
  - an optional **expiry** for its notices (SPEC-624 NTC-002; default 1 day);
  - an **owner** (the person who created it, or the Superagent's person);
  - a run **timeout** (1 s to 1 h, default 5 minutes);
  - optional **agent secrets**, by name (WCH-004).

  A project has at most 20 watches. A watch names at most 20 agent secrets. Its name is the key of
  its notices, so it never changes: a rename is a new watch. A watch is paused, active or ended. An ended watch keeps its
  history and can be resumed, which starts it again.
- **WCH-002 Where it runs.** Yulius runs the script with `bash` through the process host, in the
  sandbox's containment: the same bubblewrap boundary on the sandbox root, joined to the
  sandbox's namespace holder, that tasks and CI jobs get (ADR-0003, ADR-22). It runs in CI's
  resource group (`ci_join`). It gets no agent, no subscription, no task token, no GitHub token
  and no hooks.
  - Its working directory is the project's repository clone, when that exists.
  - `HOME` is the project's watch home, `<data>/watches/<sandbox>/home`. It is shared by the
    project's watches, kept between runs, and bound writable into the containment.
  - `YULIUS_WATCH` holds its name and `YULIUS_WATCH_STATE` its own state directory, also kept
    between runs.
  - Without containment (development, tests) it runs as a host process, as CI jobs do.
- **WCH-003 Run bounds.** Each run is bounded:
  - **time**: the watch's timeout, from the process's start, enforced by the process host's
    group kill;
  - **CPU**: `ulimit -t`, 120 s of CPU time by default as the soft limit (SIGXCPU), with a
    hard limit 2 s later (SIGKILL);
  - **memory**: `ulimit -d`, a 1 GiB data segment by default (writable private memory since
    Linux 4.7);
  - **files**: `ulimit -f`, 1 MiB per file the run writes by default. An output file that reaches
    it fails the run, whether the write was refused or killed the writer. Only the first 4,000
    characters of the output are posted.

  The limits are set as hard limits before the script starts, so the script cannot raise them
  (past the CPU limit's 2-second grace). The project's watch files (its `home`, state directories
  and runs) are outside the sandbox's disk quota, so a run that grows them past 1 GiB is stopped. A run past a bound is a failure whose reason names the bound. The values are
  the instance's (`AppConfig.watches`, not yet a server flag). At most 4 runs go at once on an instance, and 1 per
  watch. A watch that is due while its previous run still goes waits for the next tick.
- **WCH-004 Secrets only through the egress proxy.** A watch can name agent secrets of its
  project that reach runs of no person (`Holder::Nobody`: the project's and the global secrets
  switched on for it, never a personal one, never a production-deploy one). Its run gets their
  placeholders and an egress proxy credential that covers only those names, as a CI job's grant
  does. The credential closes when the run ends. A secret that is not found is left out, and the
  run's error output says so. No secret value enters the run's environment or files.
- **WCH-005 Exit codes.** After a run:
  - exit 0 with output (stdout, trimmed) posts it;
  - exit 0 with no output posts nothing;
  - exit 3 posts its output, if any, and ends the watch ("the script said it is done");
  - any other exit, a signal, a bound, or a start that failed is a failure.

  A success or exit 3 resets the failure count.
- **WCH-006 Posts are keyed notices.** A post is a SPEC-624 notice for the watch's owner in its
  project:
  - key `watch:<name>`, category `done`, tone `info`;
  - title `<name>: <first line of the output>`, detail the whole output (cut to 4,000
    characters) whenever the title does not hold all of it;
  - the output and error tail are redacted of the project's agent secrets before they are kept
    or posted;
  - link to the project's Watches page;
  - expiry the watch's own, else 1 day.

  Each post replaces the previous one (NTC-003). It reaches the owner's channels as every notice
  does (pages, the person tools, the digest, Telegram, and the bell once SPEC-624's second pull
  request merges).
- **WCH-007 Three failures, one warning.** The third failure in a row posts one warning keyed
  `watch:<name>:failing` (category `failed`, tone `error`). It says how many runs failed, and
  gives the last reason and the tail of the error output. Later failures post nothing more
  until a run succeeds. That success removes the warning notice and makes the count start again.
- **WCH-008 Delivery is retried.** A post or warning that could not be stored (the store refused
  it) is kept in the watch's record as pending, newest per key, because a newer post replaces an
  older one anyway. If that write fails too, it is kept in memory. Every scheduler tick retries
  pending posts before it starts runs, finished watches included, until they are stored. A
  pending post also survives a restart.
- **WCH-009 Schedule.**
  - An interval watch is due `every` after its last run started. A new or resumed watch runs at
    the next tick.
  - A cron watch is due at its next slot after its last run started, or after it was created or
    resumed.
  - Missed slots are never replayed: after downtime, a watch that is overdue runs once and then
    keeps its schedule.
  - The scheduler ticks every 15 s and starts the longest overdue watches first.
  - The store's claim decides whether a run starts: none of the watch goes, its project is not
    removed and, unless it is Run now, the watch is neither paused nor ended. The run uses the
    record as it is after the claim.
- **WCH-010 Survives restarts and updates.** Watches, their state and pending posts are rows in
  the store (migration 0093). A run that a restart interrupted is recorded as lost (a failure),
  its process host is ended, and its schedule counts again from the start: an interval watch runs
  at the next tick, a cron watch at its next slot.
- **WCH-014 A removed project's watches stop.** The scheduler does not see them and a run of
  them is never claimed, not even by Run now. Its going runs are cancelled with the removal, its
  pending posts are dropped, and its watch files are a removed project's leftovers.
- **WCH-011 People manage watches** (second pull request):
  - Anyone who can read the project lists its watches: name, schedule, state, next run, last
    run (time, exit, reason), last output, failure count and pending posts.
  - A writer creates one, and becomes its owner.
  - The owner edits, runs now, pauses, resumes and removes it. A project administrator may also
    pause and remove it.
  - Run now runs it once at once, also when it is paused or ended, and posts as usual.
  - Each change is audited.
- **WCH-012 The Superagent manages watches** (second pull request): MCP tools `watch_create`,
  `watches_list`, `watch_run`, `watch_pause` (and resume) and `watch_remove`, for a person's
  Superagent, acting as that person.
  - `superagent_check_set`'s description says to use a watch when a script can decide the check.
  - The docs show how to move "#479 merge-queue targets" onto a watch.
- **WCH-013 A Watches page** per project (second pull request): the list, Run now, Pause/Resume,
  Edit and Remove, and New watch.

## Plan

- [x] Watch names, schedules, outcomes and run bounds are decided in the domain
- [x] Watches are stored with their state and pending posts (migration 0093)
- [x] A watch run executes its script in the project's sandbox under its bounds
- [x] A run's exit code posts, replaces, warns or ends the watch
- [x] Posts that could not be stored are kept and retried
- [x] The scheduler runs due watches and picks them up again after a restart
- [x] Review round 1 of the watch core: every finding fixed test-first
- [ ] Independent review round 2 of the watch core, every finding answered
- [ ] People create, edit, run, pause and remove watches over HTTP
- [ ] The Superagent manages watches through MCP tools, and checks point to watches
- [ ] The project's Watches page offers Run now, Pause/Resume, Edit and Remove
- [ ] Product, architecture, API and operations docs describe watches
- [ ] Independent review of the API, tools and page, every finding answered

## Acceptance criteria

- **AC-1** (WCH-009) An interval watch of 5 minutes runs at the first tick, not again 4 minutes
  later, and again once the clock passes 5 minutes. A cron watch `0 9 * * *` runs once the clock
  passes 09:00 and not again that day.
- **AC-2** (WCH-005, WCH-006) Exit 0 with output posts `watch:<name>` to the owner, and a second
  run's output replaces it (one notice, the new title). Exit 0 with no output posts nothing and
  leaves the previous notice. Exit 3 posts and ends the watch, which the next tick does not run.
- **AC-3** (WCH-007) Three failing runs post one `watch:<name>:failing` notice, a fourth adds
  nothing, and a successful run removes it and posts its own output.
- **AC-4** (WCH-008) While the store refuses notices (an injected failure), a run's post stays
  pending on the watch. Once the store accepts notices again, the next tick posts it once.
- **AC-5** (WCH-010) After a new server process starts on the same data directory, the watch is
  listed with its state and runs on schedule. A run left `running` by the old process is recorded
  as lost and runs again.
- **AC-6** (WCH-003) A busy loop ends by its CPU bound well before its timeout. A script that
  allocates past the memory bound fails. A script that writes past the output bound fails. A
  `sleep` past the timeout is killed. Each has a reason that names its bound. A script cannot
  raise its own limits.
- **AC-7** (WCH-002, WCH-004) A contained run's plan uses the sandbox root, the holder join in
  CI's group and the watch home as its writable path. A run granted `API_KEY` gets its placeholder
  and the proxy variables, never the value.
- **AC-8** (WCH-011, WCH-012, WCH-013) is listed with the second pull request's tests.

## Test plan

- Domain unit tests (`yulius-domain/src/watch.rs`): name and schedule parsing and bounds, the
  due time for interval and cron (AC-1), the outcome table for every exit code and failure count
  (AC-2, AC-3), and the reason for each bound (AC-6).
- Store tests (`yulius-store/src/watches.rs`): create, unique name per project, state updates,
  pending posts newest per key, runs left running found at start (AC-5).
- Integration tests in the `yulius` binary (`tests/watches.rs`), with the real process host
  uncontained and the test clock:
  - the schedule fires (AC-1);
  - exit-code semantics and keyed replacement (AC-2);
  - three failures make one warning (AC-3);
  - a retry after an outage, with a SQLite trigger refusing `notices` inserts (AC-4);
  - survival across a restart, with a second harness on the same data directory (AC-5);
  - the run bounds enforced, with small limits from the configuration (AC-6);
  - the contained plan through a recording process host, and the secret placeholders (AC-7).
- Mutation by hand: undo the replacement key, the failure threshold, the pending retry, the
  `ulimit` lines and the lost-run recovery, and record which tests fail.

## Decisions

- **A watch is a new record, not a kind of automation or check.** An automation's whole contract
  is the task it creates (run rows linked to tasks, start choices, harness, workflow), and a check's
  is the Superagent's prompt box. A watch has neither: it runs a process and posts a notice. Folding
  it into either would give that record a second meaning in every reader. ADR-0465.
- **It runs as a sandbox job, like CI, not as a person's shell.** The containment boundary is the
  sandbox's, which every task and CI job shares. A person's identity (their Git account, their
  personal secrets) would let anyone who can later edit the watch act as that person. Automations
  hold no person's credential for the same reason (`Holder::of_task`). A watch's secrets are
  named, and resolved as a CI job's are (`Holder::Nobody`).
- **The egress credential reuses CI's caller kind.** A watch run is a job of no person with a
  named grant, and that is exactly `EgressCaller::Ci`'s rule. The credential names the run
  `watch:<name>`, so the audit actor is `ci:watch:<name>`. A new caller kind would repeat the same
  rule in every match of the proxy, beside a parallel change to the proxy's rules (#611).
- **Bounds are `ulimit`s set by a fixed bash wrapper, plus the process host's time limit.** The
  limits travel as positional arguments and the script as its own `bash -c` argument, so nothing
  is spliced into shell text. The other options:
  - a cgroup per run: the isolation creates per-job groups only for CI jobs, and those do not
    exist without isolation, so tests could not prove the bound;
  - `prlimit` on the spawned process: it races the script's start.

  RLIMIT_DATA rather than RLIMIT_AS, because runtimes such as Node reserve large unwritable
  address ranges that are not memory.
- **Posts go to the owner only.** A watch is someone's question ("tell me when…"). Everyone in
  the project sees the watch on its page, but only its owner is notified. Posting to the whole
  project can come later as an option.
- **Category `done` for reports, `failed` for the warning.** Telegram choices are by these
  categories. A report is news that needs no decision, and a broken watch is a failure. Adding a
  new category would change every person's Telegram choices and the notifier, which SPEC-624's
  open second pull request is changing.
- **Pending posts are kept newest per key.** A newer post with the same key would replace the
  older one on arrival, so retrying an older one only adds churn. This also bounds the queue to
  two entries per watch.
- **`superagent_check_set` gets no script variant.** The tool description and the docs point to
  `watch_create` instead: one scheduled-script mechanism, not two.
- **Two pull requests.** The core (domain, store, runner, scheduler, notices) comes first, and the
  API, MCP tools, page and docs second. Each then stays one concern of reviewable size.

## Evidence

### First pull request: the watch core (steps 1 to 6)

- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^watches::/)'`: 9
  passed. The tests cover the schedule (AC-1), the exit codes with keyed replacement (AC-2),
  three failures making one warning (AC-3), the warning's text, a retry after an outage (AC-4,
  a SQLite trigger refusing `notices` inserts), survival across a restart (AC-5, a new `App` on
  the same store), the run bounds (AC-6), a contained plan with secret placeholders (AC-7) and
  refused definitions.
- The whole `yulius::main` binary: 2,648 passed, 24 skipped (log in the worktree's ignored
  `target/watch-evidence/main-run.log`).
- `scripts/cargo nextest run -p yulius-domain -p yulius-store -p yulius-app`: 1,519 passed. This
  includes the domain tests (`watch::tests`, 9) and the store tests (`watches::tests`, 3) with the
  pinned migration list.
- `scripts/cargo clippy -p yulius-app -p yulius-domain -p yulius-store -p yulius --all-targets
  -- -D warnings`: clean. `scripts/dev quick` and `python3 scripts/repository_tests.py` (34
  affected file-contract tests) passed.
- Mutation by hand. Each line below was undone or changed, the tests were run, and the line was
  restored:
  - Without the wrapper's `ulimit` line, `run_bounds_are_enforced` failed: the busy loop ran to
    its 20 s timeout.
  - Without `ulimit -d`, it failed: the 100 MB string was built and posted.
  - Without `ulimit -f`, it failed: the state file grew past 64 KiB.
  - With soft-only limits (`ulimit -St`), it failed: the script raised its own CPU limit.
  - With a report key that changes per run, five tests failed: the exit codes, the warning, the
    outage, the restart and the schedule.
  - With a warning on every failure past three, `three_failures_make_one_warning` and the domain
    test `three_failures_warn_once_and_a_success_clears` failed.
  - Without the tick's retry of pending posts, `a_post_is_retried_after_an_outage` failed.
  - With the start's recovery of lost runs skipped, `watches_survive_a_restart` failed.
- Security scan: Vidoc is switched off for this project in Yulius, so the scan reports disabled,
  not passed.
- Not tested here: a real bubblewrap run joined to a real holder. The contained plan is checked
  through a recording process host with a fake holder, as the CD tests check theirs, and the
  bounds are proven on the host. Also not tested: the bell showing a watch's notice, which waits
  for SPEC-624's second pull request.

### Review round 1 of PR #1713 (head `cc8825af8`): request changes, all answered

All findings were in this pull request's own code. Each fix has a test, and each test was seen
failing with its fix undone (by hand, one at a time):
- **A removed project's watches kept running (major).**
  - `all_watches` and the run claim leave out removed projects, so even Run now is refused.
  - The removal cancels going runs (`watch_cancel_project` in `end_removed_sandbox`).
  - Pending posts of a removed project are dropped, and `<data>/watches/<sandbox>` is a removed
    project's leftover.
  - Test: `a_removed_projects_watches_stop`. It fails without the cancel (the run hangs) and
    without the claim's filter (Run now runs).
- **Four clauses no test could fail on (major).**
  - `a_runs_secrets_reach_it_only_through_its_proxy_credential`: with egress on, the credential
    is `ci:watch:keyed`, intercepts only the named secret's host and ends with the run. It fails
    without `egress_env`.
  - `a_run_end_the_store_refused_is_written_by_the_next_tick`: a trigger refuses the end's
    update. It fails without the in-memory buffer.
  - `runs_at_once_are_bounded_and_the_longest_overdue_goes_first`: `max_runs = 1`. It fails
    without the sort by due time.
  - `watches_survive_a_restart` now drops a real run's future and checks that its `sleep`
    process ends at the next start. It fails without `watch_end_lost`.
- **Minor findings:**
  - The detail is the whole output whenever the title cannot hold it (fails with the old
    newline-only rule).
  - Due watches start longest overdue first.
  - The run claim checks `paused` and `ended_reason` unless Run now (`forced`), and the run
    re-reads the record after the claim (store test `claims_skip_removed_projects_and_paused_watches`).
  - The watch files have a 1 GiB disk bound (`a_run_that_fills_the_watch_files_is_stopped`, fails
    with the bound lifted) and are removed-project leftovers.
  - The CPU soft limit is below the hard one, so SIGXCPU fires and the reason reads "it used its
    CPU time" (fails with soft equal to hard).
  - `last_output`, `last_errors` and the pending posts are redacted before they are kept (fails
    without the redaction).
- **Nits:**
  - The error file's length comes from a handle opened before the start.
  - The script runs as `bash -c -- "$7"` (a script `--version` now fails with 127 instead of
    printing bash's version, which the test checks).
  - At most 20 secret names, without duplicates.
  - The link uses the slug only when it is URL-safe.
  - Recovery uses `watch_due_again`, which never un-pauses.
  - Names never change (`update_watch` keeps the name, store test), so notices keep their key.
- `scripts/cargo nextest run -p yulius -p yulius-store -E '(binary_id(yulius::main) &
  test(/^watches::/)) | (package(yulius-store) & test(/watches::/))'`: 19 passed.
