---
yulius-task: 63aa8551-e6b8-4a74-bf89-2c12155c2ed0
status: done
---

# SPEC-461: Claude Code in sandboxes always knows the models Yulius offers

Status: implemented
Date: 2026-11-11
Owner: Yulius task #443
Scope authorization: the owner's goal for task #443 (critical, found on production 2026-11-11),
quoted under Context. Amends SPEC-421 CCU-001 (the default channel), CCU-002 (what a release must
be before it becomes current), CCU-003 (a running process keeps its version) and CCU-006 (the
doctor line). Decision record:
[ADR-0293](../../docs/decisions/0293-claude-code-follows-latest-and-must-know-the-default-model.md),
which amends [ADR-0266](../../docs/decisions/0266-yulius-keeps-claude-code-current-in-its-data-directory.md).

## Goal

A sandbox never runs a Claude Code that does not know Yulius's default model. Yulius follows
npm's `latest` release, checks each release against the models the composer offers before it
becomes current, and says loudly (Settings, Instance; `yulius doctor`) when the Claude Code in use
does not know the default model, with the fix. A superadmin sees the channel, the version in use
and the newest release, checks now and changes the channel in Settings. A running task moves to
the new Claude Code at a turn boundary in the same session, and the UI says which tasks still run
an older one.

## Context

- **The incident (production, 2026-11-11).** The default model is `claude-fable-5-2` since
  2026-11-10 (ADR-0289, `--model claude-fable-5-2`). The Claude Code Yulius keeps current followed
  npm's `stable` dist-tag (`YULIUS_CLAUDE_UPDATES`, default `stable`): 2.1.311, while `latest` was
  2.1.322. The 2.1.311 binary does not contain `claude-fable-5-2` at all, only `claude-fable-5-1`:
  the API answered as `claude-fable-5-2` because the id is passed explicitly, but the session's
  `/model` picker shows "Fable 5.1" for Fable, and the `fable` alias, and anything Claude Code
  resolves by itself (subagents, a person picking a model in the session), means 5.1. Patched by
  hand: `npm install -g @anthropic-ai/claude-code@2.1.322` on the host (new runs get the newer of
  the host's install and Yulius's copy) and a drop-in `YULIUS_CLAUDE_UPDATES=latest`. Running
  sessions keep 2.1.311 until their next start.
- **Measured here, 2026-11-11.** `GET /-/package/@anthropic-ai%2Fclaude-code/dist-tags` answers
  `{"stable":"2.1.311","latest":"2.1.322","next":"2.1.322"}`. The installed 2.1.322 binary
  (341 MB) holds `claude-fable-5-2` 26 times as plain text (its model tables are JavaScript
  source), `claude-fable-5-1` 41, `claude-opus-5-5` 72, `claude-sonnet-5-5` 39, `claude-haiku-4-5`
  51 times, and an id it does not know (`claude-fable-5-3`) 0 times. So a build can be checked for
  a model id by reading it, as SPEC-421 already does for `VERSION:"X.Y.Z"`, without running it.
- **What exists (SPEC-421, ADR-0266).** `crates/yulius/src/wiring/claude_cli.rs` checks the
  channel's dist-tag about every hour, downloads the platform package, verifies its sha512, ELF
  machine and embedded version, installs it under `<data>/claude-code/versions/<v>` and points
  `current` at it; every run joining an isolated sandbox binds the newer of the host's package and
  `current` (`agent_cli_binds`). The channel is a start-up argument only; nothing but the log and
  one `yulius doctor` line (always `ok`) shows the state.
- **The models Yulius offers** are listed in `crates/yulius-server/web/assets/models.js` (the
  composer) and the default is `DEFAULT_CLAUDE_MODEL` (`crates/yulius-domain/src/harness.rs`); the
  server has no list of them.
- **Task info** (`GET /api/tasks/{tid}`, `pages/taskinfo.js`) lists the models its sessions'
  transcripts name (`Metrics.models`, the model that answered), but neither the model the task
  asked for nor the Claude Code version. Transcript lines carry `"version":"2.1.322"`; the harness
  parses it (`Observed::SessionMeta.version`) and drops it.
- **Restarting a session without losing it** exists: at a turn boundary the pool moves a task to
  another subscription by ending the process and starting `claude --resume <session>`
  (`hook_stop`, `rotate`, `switch_task_now`; SPEC-034, SPEC-447), never while background shells or
  agents run (they would be lost).

## Requirements

| ID | Requirement |
| --- | --- |
| CCM-001 | **Channel default.** `YULIUS_CLAUDE_UPDATES` (`--claude-updates`) defaults to `latest`; `stable` and `off` stay. |
| CCM-002 | **Offered models.** The server knows the Claude models the composer offers (one list, the default first; a test keeps `models.js` equal to it). A Claude Code build *knows* a model when its binary holds the model id as text not followed by a letter, a digit or a hyphen (so `claude-fable-5-2` is not read in `claude-fable-5-20`, nor `claude-fable-5` in `claude-fable-5-2`; in 2.1.322 every offered id stands that way at least 26 times, mostly before a quote). The binary is read, never run. |
| CCM-003 | **Verified before current.** A downloaded release is checked for every offered model in the same pass as its version. One that does not know the default model is refused: nothing is installed, `current` stays, no temporary directory is left, and the refusal names the release and the model. One that knows the default model but not another offered model becomes current and the gap is reported (CCM-006). A refused release is not downloaded again while the server runs (a published version never changes). |
| CCM-004 | **`stable` never falls behind the default model.** On `stable`, when neither the version runs get nor stable's release knows the default model, `latest`'s release is taken instead, under CCM-003, and the report says so. On `latest` there is nothing newer to take: the refusal is reported. On `off` nothing is installed. |
| CCM-005 | **The version in use is checked too.** The Claude Code runs get (the newer of the host's install and Yulius's `current`) is read for the offered models when the server starts, after every check and when the channel changes (cached per file while it does not change). |
| CCM-006 | **Reported.** After every check the server records the channel, the time, npm's `latest` and `stable`, what the check did (installed, up to date, refused with the release and its missing models, or failed with the reason), the version in use, where it comes from and the offered models it does not know, in `<data>/claude-code/status.json` (for `yulius doctor`) and in memory (for the Instance page). A refusal is logged as a warning and a version in use that does not know the default model as an error. |
| CCM-007 | **Doctor.** `yulius doctor --contained` prints "Claude Code in sandboxes" as `FAIL` when the version runs get does not know the default model, naming the version, the model and the fix (what Yulius does at its next check, "Check now" in Settings, and `sudo npm install -g @anthropic-ai/claude-code@latest` on the host); as `warn` when it lacks another offered model or the last release was refused; else `ok`. It names the channel in effect: the one chosen in Settings (read from the status file) or `YULIUS_CLAUDE_UPDATES`. |
| CCM-008 | **Instance page.** Settings, Instance has a "Claude Code" card (superadmin): the channel and where it is set, the version in use and where it comes from, the host's version, npm's `latest` and `stable` at the last check, when it was checked and what it did; a danger banner with the fix while the version in use does not know the default model; a warning for another unknown offered model, a refused release and a failed check; "Check now"; the channel as a choice of Latest, Stable and Off; and the tasks that still run an older Claude Code (CCM-013). |
| CCM-009 | **Channel in Settings.** `PUT /api/admin/claude-code/channel {"channel": "latest"\|"stable"\|"off"\|null}` (superadmin) stores the channel in the database (`null`: back to `YULIUS_CLAUDE_UPDATES`; a body without the field is refused, 422). A stored channel wins over the unit's. It applies at once, without a restart: the next check runs now, runs started from then on bind as the channel says (`off`: the host's install), and it is audited `claude_code.channel`. An unknown channel is refused (422). |
| CCM-010 | **Check now.** `POST /api/admin/claude-code/check` (superadmin) starts a check at once and answers 202 with the view saying it runs; one check runs at a time (a second request joins the running one). The hourly check and the first check 2 minutes after a start stay. `GET /api/admin/claude-code` answers the view. |
| CCM-011 | **Sessions record what ran.** Each session of a task records the Claude Code version its transcript's prompt records name (`metrics.cli_version`), beside the model that answered (kept already), as soon as a line names it. |
| CCM-012 | **Task info.** A task's info shows the model it asks for with its effort (marked "default" when the task names none), the models that answered, and the Claude Code version of its latest session, with "moves to X when its turn ends" while its running session is on an older one; each row of its Sessions table shows what that session ran on (its Claude Code version and model). `GET /api/tasks/{tid}` carries them (`model`, `model_default`, `effort`, `cli_version`, `cli_moves_to`; per attempt `model`, `cli_version`). |
| CCM-013 | **A running task moves.** When a turn of a running task ends and its session runs an older Claude Code than runs get now, Yulius restarts the session on the newer one in the same Claude session (`claude --resume`, same subscription, the path a subscription move takes at a turn boundary) and says so in one line in its terminal. A session that works on after the turn (a gate's block, a goal that is set and not yet achieved) moves at that turn's Stop hook and gets the gate's reasons, and its goal again, as after a subscription move; a goal task that is Done keeps its session at a follow-up's end (its goal would be typed again); a prompt-mode session that goes idle moves once the turn has settled (its pull request opened, chains advanced), with nothing typed, and is idle again. Not while its background shells, agents or monitors run, a shell that only runs `yulius wait` included (its news would never arrive), not while a person typed into it in the last minute or paused it, not while an instance update holds the runs, not within the cooldown of a refused switch, and once per version per task. A restart that cannot start now says nothing and is tried at a later turn's end. A session that could not continue on a version stops further moves to that version (the card says so). A task that is idle keeps its version until its next turn ends or its next start, and so does a run without hooks. The Superagent, shells and CI jobs get the newer version when they next start, as before. A restart for a newer Claude Code is no subscription switch in the task's info. |

## Plan

- [x] Spec and ADR written, the incident reproduced as a failing test
- [x] A release that lacks the default model is refused, and `latest` is the default channel
- [x] The version in use is checked and reported, and the doctor fails loudly on it
- [x] The channel is a stored setting that applies at once, with Check now
- [x] Settings, Instance shows the Claude Code card
- [x] Sessions record their Claude Code version and task info shows it with the models
- [x] A running task moves to the new Claude Code at its turn boundary
- [x] Operations guide, API reference, architecture and changes updated
- [x] Independent review round 1 answered, every finding fixed with its test
- [x] Review approved on the final head and every gate green

## Acceptance criteria

- AC1 (CCM-003). With the registry offering a release whose binary lacks `claude-fable-5-2`, a
  check installs nothing, `current` and the installed versions are as before, no temporary
  directory is left, and the error names the release and `claude-fable-5-2`.
- AC2 (CCM-001). `Channel::default()`, `IsolationArgs::default()` and both commands' `--claude-updates`
  default are `latest`.
- AC3 (CCM-004). On `stable` with stable's release lacking the default model and `latest`'s knowing
  it, one check installs `latest`'s; with both lacking it, nothing is installed and both are named.
  With the version in use knowing the default model, `stable` stays on stable's release.
- AC4 (CCM-003). A release that knows the default model but lacks `claude-haiku-4-5` is installed,
  and the status names the missing model.
- AC5 (CCM-007). With runs getting a Claude Code that lacks the default model, the doctor's line is
  `FAIL` and names the version, `claude-fable-5-2` and the fix; with one that knows every offered
  model it is `ok`; with the channel set in Settings it names that channel.
- AC6 (CCM-009, CCM-010). `PUT …/channel` stores the channel, applies it without a restart (a join
  made after `off` binds the host's package), is refused for others and for an unknown channel;
  `POST …/check` answers 202 and the view then shows the check's result; both are superadmin-only.
- AC7 (CCM-008). The Instance page shows the card: versions, channel, the danger banner with the
  fix when the default model is unknown, a refused release, the channel choice, "Check now" and
  the tasks on an older version (e2e against the mock).
- AC8 (CCM-011, CCM-012). A session whose transcript names a version shows it in the task's detail
  (`AttemptView.cli_version`, `TaskDetail.cli_version`), also once the run has ended; task info
  shows the model asked for, the models that answered and the version.
- AC9 (CCM-013). A running task whose session runs 2.1.311 while runs get 2.1.322 is restarted at
  the end of its turn: a second run of the same Claude session and subscription, launched by
  resume, with one terminal line, idle again afterwards (prompt mode) or with its goal set again
  and achieved (goal mode); nothing is restarted again at the next turn end, nor when the
  versions are equal, nor under a person who typed in the last minute; background work, a
  refused switch and a version a session could not continue on keep it where it is.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: the channel default (AC2); which channel falls
  forward to `latest`; where a model id ends; the gate (default model) and the reported gap; the
  problem's and the fix's words per channel; when a session moves (`moves_now`: every refusal of
  AC9). `crates/yulius-domain/src/harness.rs`: the offered list starts with the default.
- Unit, `crates/yulius/src/wiring/claude_models.rs`: the streaming model scan (ids across chunk
  boundaries, at the end of the file, followed by a digit, a letter or a hyphen, absent) and its
  per-file memory. `crates/yulius-harness/tests/observe.rs`: the run's CLI version is its latest
  prompt record's, never a sub-agent's.
- Integration, `crates/yulius/tests/claude_updates.rs` (the registry stub of SPEC-421, whose
  binary stand-in now embeds the offered model ids): AC1, AC3, AC4, the status file and the
  in-memory record after each outcome, AC5 (the doctor's level and text), the composer's list
  equal to the server's (`models.js`), the declared channel default, a channel change applied at
  once to binds, "Check now" waking the loop, and every SPEC-421 test unchanged in meaning.
- Integration, `crates/yulius/tests/claude_code_admin.rs`: AC6 over the HTTP API with a fake
  adapter (the superadmin gate, the store round trip of the channel, the audit entries, an unknown
  channel, the view's problem and fix, 202 on "Check now", a server without isolation).
- Integration, `crates/yulius/tests/claude_code_moves.rs` (real processes: the fake agent runs
  Claude Code's hooks against a served app, and its transcript names the version a file holds
  when it starts): AC8; AC9 in prompt mode and in goal mode; the typing refusal; and the idle
  state after a person's move of an idle task (the defect met on the way).
- e2e, `e2e/tests/claude-code.e2e.ts` (mock): AC7 and AC8's task info rows.
- Not testable here: a real download from npm and a real 340 MB binary in CI (the stub's stand-in
  is small; the scan was run by hand against the installed 2.1.322, see Evidence); the move of a
  real Claude Code session (the fake agent stands in for it); the background-work refusal with
  real processes (it reuses `background_work_now`, covered by the task-done suites, and the
  decision is unit-tested).

## Decisions

- **One pull request, larger than 400 lines.** The owner asked for these outcomes as one critical
  task with one "done" (merged into `dev`). They share one status model and one port: the model
  check feeds the doctor, the Instance card and the list of tasks on an older version, so a split
  would ship a check nobody can see, or a card with nothing behind it. Commits follow the Plan's
  steps for review.
- **The default model is the hard gate; the other offered models are reported.** A release without
  the default model is the incident and never becomes current. Refusing a release for any offered
  model would stop every update the day a retired model's id leaves Claude Code's binary while the
  composer still lists it, which is the opposite failure (falling behind). So such a release is
  taken and the gap is said, on the card and in the doctor.
- **Read, not run** (as ADR-0266): the model ids are plain text in the binary; running a fresh
  download as the service user would hand it the database and the vault key.
- **`latest` by default** (reverses ADR-0266's default): `stable` lags about a week, which is how
  the default model outran it within a day. The check of CCM-003 is what `stable` was a proxy for
  here, and `stable` and `off` stay one click away in Settings.
- **`stable` falls forward to `latest` only for the default model**, not for any gap: `stable` is a
  person's choice of fewer regressions, and only a Claude Code without the default model is worth
  overriding it.
- **The channel is stored in the database**, not in a file under `claude-code/`: an instance
  export leaves that directory out, and a setting must move with the instance. The status file
  there is derived state for the doctor, which opens no database.
- **The move reuses the subscription switch** (same subscription, reason `claude_code`) instead of
  a new restart path: its preflight, its hand-over to a stop or close that overtakes it, and its
  refusal to end background work are the hard part, and they exist and are tested.
- **Idle sessions are not restarted when a release arrives.** A resumed goal session gets its
  goal typed again, which would start work nobody asked for in an idle task; a session moves at
  the end of a turn: a goal session at the Stop of a turn it works on after (the goal typed again
  is what happens next anyway), a prompt-mode session once its turn has settled, with nothing
  typed.
- **A clean turn end settles before an idle session moves.** Moving at the Stop hook, as a
  subscription move does, skips what a clean turn end does (the task's activity, its pull request,
  its chains). Rare for a subscription move, it would happen to every prompt-mode task once per
  release, so the move runs after `settle_clean_turn`'s work.
- **Runs without hooks do not move.** Their turn end is read from the transcript after the fact;
  every supported Claude Code runs Yulius's hooks, so only test runs lack them.
- **The version is the transcript's word**, stored in `metrics.cli_version` through its own store
  call: `AttemptRecord` and `MetricsRecord` are built in dozens of places (and in parallel pull
  requests), so neither got a field.
- **The move decision is a pure function** (`agent_cli::moves_now`), so each refusal is unit-tested
  without processes; the app only gathers its facts, the cheap ones first.
- **Met on the way, fixed here:** a prompt-mode session that was idle read "working" after a move
  that types nothing, also a person's move to another subscription (SPEC-447 MOV-002): the launch
  marks every run working and no turn ended to say otherwise. The switch now puts such a session
  back to idle (`rest_after_idle_switch`); the move of CCM-013 depends on it.
- **Review round 1** (independent reviewer, fresh session, at beb8fca9: request changes; twelve
  findings and five nits, all own code, all answered, each fix with a test that fails without it
  where a test can show it):
  1. (high) An idle session whose only background shell ran `yulius wait` was restarted and lost
     its wait. A wait now counts as background work for the move (`background_or_wait_now`);
     `a_session_with_background_work_or_a_yulius_wait_is_not_restarted` (real processes).
  2. A restart counted as a subscription switch in analytics: the query leaves `claude_code`
     out (`analytics` cohort test).
  3. The breaker had no server-side test:
     `a_version_a_session_could_not_continue_on_gets_no_other_session` (a launch that fails after
     the old run ended).
  4. Two lines for a move, said before the preflight, and "Switching subscriptions was not
     possible" for a refusal: one line, from the switch once it is under way; a refused restart
     says and records nothing.
  5. "Checking" could stick when requests arrived together (wake-ups do not stack): the loop
     runs the follow-up itself (`requests_that_arrive_together_do_not_leave_it_checking`).
  6. Typing, a person's pause and an update hold are read again after the probes (the typing
     case through the race point `ClaudeMoveProbed`).
  7. A Done goal task got its goal typed again at a follow-up's Stop: the move there needs a
     goal that is open and unmet (`a_done_goal_task_keeps_its_session_after_a_follow_up`).
  8. The fix promised a check and the host's install when runs already get npm's newest: it
     says no release knows the model (`the_doctor_says_when_no_release_knows_the_default_model`,
     the admin view test).
  9. A refused release was downloaded again at every check: remembered while the server runs.
  10. Tests waited on pauses: they wait on the race points `ClaudeMoveSettled` and `StopClean`.
  11. Task info showed the old version after an idle move: the restarted run counts as on the
      version it was sent to until its transcript names one.
  12. Docs: this Evidence section, the status, and ADR-0293's consequence (Stable does not move
      `current` back; Off does change what runs get).
  Nits: `PUT …/channel` needs the field; the doctor leaves out a refusal the host's install has
  overtaken; a version that is not plain is not waited for; the card redraws only on a change
  (focus and the alert stay); what was sent to older versions is forgotten at the next move.
- **Migration `0050_claude_code.sql` is schema version 51.** Its number was reserved first, but
  SPEC-459's `0051_instance_limits.sql` merged first as version 50; a version is a migration's
  place in the list, so this one goes after it.
- **Not from this change:** `first_message::{a_draft_typed_during_the_opening_turn_is_left_alone,
  focus_and_mouse_reports_do_not_keep_the_goal_out,
  trust_words_after_the_session_started_are_no_trust_prompt}` and
  `merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again` fail the same way on
  the base commit 911e3127 (run there in a second worktree); issue #1154 tracks them (they fail
  in a task sandbox and pass in Yulius CI).

## Evidence

- `scripts/dev ci` at 457d02f7 (the review fixes and `dev` up to bbd65665 merged): exit 0.
  Repository contracts, check, fmt, clippy `-D warnings` on Linux and for macOS, nextest 3402
  passed / 16 skipped, doctests, `cargo deny` (advisories, bans, licenses, sources ok), `cargo
  shear`, doc. Earlier runs passed at a0f7ae35 (3318 tests) and e22a0d8a (3325). The commit after
  457d02f7 changes this file only.
- The incident as a test, seen failing first:
  `claude_updates::a_release_that_lacks_the_default_model_is_refused` answered
  `Ok(Installed(2.1.311))` on the base (a release without `claude-fable-5-2` became current); it
  passes with the check.
- Tests added or changed (all pass):
  - `yulius-domain` `agent_cli` (13 unit tests): the default channel, the fall-forward tag,
    where a model id ends, the gate and the reported gap, the problem and fix sentences,
    `moves_now` with each refusal.
  - `yulius` lib `wiring::claude_models` (5): the streaming scan and its per-file memory.
    `yulius-harness` `observe`: the run's CLI version.
  - `claude_updates` (21): AC1 with the refusal remembered, AC3 in four scenarios, AC4, AC2, the
    composer's list equal to the server's, the status file and record after refused, installed,
    up-to-date and failed checks, AC5 with the unknown-version and "no release knows it"
    sentences, a channel change and "Check now" without a restart, requests that arrive together,
    and SPEC-421's tests.
  - `claude_code_admin` (3): AC6, the view's problem and fix, a body without the channel.
  - `claude_code_moves` (8, real processes): AC8; AC9 in prompt and goal mode; the typing
    refusals; a background shell and a `yulius wait`; a Done goal task's follow-up; the breaker;
    the idle state after a person's move.
  - `analytics`: a restart is no subscription switch.
  - e2e `claude-code.e2e.ts` (6): AC7 and the task info rows, no WCAG violations; with
    `update`, `sandbox-docker`, `responsive` and `text-fits` (the same page, after merging
    SPEC-459's limits card beside it): 88 passed.
- Mutations, each caught by a test and reverted: `stable` never falls forward; the version in use
  always counts as knowing the default; `stable` falls forward whenever its release was refused;
  a `yulius wait` not counted as background work; typing not read again after the probes; the
  breaker never set; the restarted run's version not recorded; a goal session moved at every
  Stop.
- Seen failing first: `moves::an_idle_session_is_idle_again_after_a_person_moves_it` and
  `moves::a_running_task_moves_to_the_new_claude_code_when_its_turn_ends` ended "Running,
  Working" before `rest_after_idle_switch`.
- Stress: the move tests and the timing-dependent updater tests 6 times over, all passed.
- The real binary: the scan run by hand over the installed Claude Code 2.1.322 (326 MB) found
  every offered model in 30 ms (release build; 447 ms debug) and read the whole file for an id
  no build knows in about 120 ms (1 s debug).
- The real doctor on this sandbox (`yulius doctor --contained --data <empty dir>`): `ok    Claude
  Code in sandboxes 2.1.322 from the host (/usr/local/lib/node_modules/@anthropic-ai/claude-code);
  Yulius installs a newer release of npm's latest (YULIUS_CLAUDE_UPDATES=latest)`, in 0.8 s.
- The registry on 2026-11-11: `{"stable":"2.1.311","latest":"2.1.322","next":"2.1.322"}`.
- The UI in the sandbox browser (mock): the card in its incident, gap and healthy states.
- Whole e2e suite once (before the review fixes): 905 passed, 2 skipped, 3 failed in files this
  change does not touch: `agent-panel` and `queued-messages` (one keyboard-focus test each) passed
  when rerun alone; `places` "the editor keeps open files…" was the load flake of #1157.
- Review: round 1 (fresh session, at beb8fca9) requested changes, answered above; round 2 is
  made on the head that holds this section, and its verdict goes with the merge.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, not passed.
- Not tested: a real download and switch on a deployed instance; the move of a real Claude Code
  session (the fake agent stands in), including what a real `--resume` writes as its version;
  a refused move's silence in a terminal; a Claude Code whose binary no longer names its models
  as plain text (the check would refuse every release, which the card and the doctor would say).
