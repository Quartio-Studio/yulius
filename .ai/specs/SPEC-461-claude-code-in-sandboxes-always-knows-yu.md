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
| CCM-007 | **Doctor.** `yulius doctor --contained` prints "Claude Code in sandboxes" as `FAIL` when the version runs get does not know the default model, naming the version, the model and the fix (what Yulius does at its next check, "Check now" in Settings, and `sudo npm install -g @anthropic-ai/claude-code@latest` on the host; or, when npm's newest release does not know the model either, that no release knows it yet, which is said only from the server's own read of the registry in the last two hours on a channel that reads it: not on `off`, not after a failed check, as on the card); as `warn` when it lacks another offered model or the last release was refused; else `ok`. It names the channel in effect: the one chosen in Settings (read from the status file) or `YULIUS_CLAUDE_UPDATES`. |
| CCM-008 | **Instance page.** Settings, Instance has a "Claude Code" card (superadmin): the channel and where it is set, the version in use and where it comes from, the host's version, npm's `latest` and `stable` at the last check, when it was checked and what it did; a danger banner with the fix while the version in use does not know the default model; a warning for another unknown offered model, a refused release and a failed check; "Check now"; the channel as a choice of Latest, Stable and Off; and the tasks that still run an older Claude Code (CCM-013). |
| CCM-009 | **Channel in Settings.** `PUT /api/admin/claude-code/channel {"channel": "latest"\|"stable"\|"off"\|null}` (superadmin) stores the channel in the database (`null`: back to `YULIUS_CLAUDE_UPDATES`; a body without the field is refused, 422). A stored channel wins over the unit's. It applies at once, without a restart: the next check runs now, runs started from then on bind as the channel says (`off`: the host's install), and it is audited `claude_code.channel`. An unknown channel is refused (422). |
| CCM-010 | **Check now.** `POST /api/admin/claude-code/check` (superadmin) starts a check at once and answers 202 with the view saying it runs; one check runs at a time (a second request joins the running one). The hourly check and the first check 2 minutes after a start stay. `GET /api/admin/claude-code` answers the view. |
| CCM-011 | **Sessions record what ran.** Each session of a task records the Claude Code version the records of its own thread in its transcript name (`metrics.cli_version`; Claude Code writes its version on every record, and a sub-agent's names nothing for the session), beside the model that answered (kept already), as soon as a line names it: a session resumed with nothing typed names it with the first record it writes, not with its next prompt. A recount of a run's transcript records it too. At most 64 bytes of what a record names are kept. |
| CCM-012 | **Task info.** A task's info shows the model it asks for with its effort (marked "default" when the task names none), the models that answered, and the Claude Code version of its latest session, with "moves to X when its turn ends, or at its next start" while its running session is on an older one; each row of its Sessions table shows what that session ran on (its Claude Code version and model). `GET /api/tasks/{tid}` carries them (`model`, `model_default`, `effort`, `cli_version`, `cli_moves_to`; per attempt `model`, `cli_version`). |
| CCM-013 | **A running task moves.** When a turn of a running task ends and its session runs an older Claude Code than runs get now, Yulius restarts the session on the newer one in the same Claude session (`claude --resume`, same subscription, the path a subscription move takes at a turn boundary) and says so in one line in its terminal. A session that works on after the turn (a gate's block, a goal that is set and not yet achieved) moves at that turn's Stop hook and gets the gate's reasons, and its goal again, as after a subscription move; a goal task that is Done keeps its session at a follow-up's end (its goal would be typed again); a prompt-mode session that goes idle moves once the turn has settled (its pull request opened, chains advanced), with nothing typed, and is idle again. Not while its background shells, agents or monitors run, a shell that only runs `yulius wait` included (its news would never arrive), not while a person typed into it in the last minute or paused it, not while an instance update holds the runs, not within ten minutes of a refused subscription switch or of a refused restart, and once per version per task. Deciding takes at most 3.5 s of the Stop hook's 5 s (one wait for the turn's lines, one look at the session's processes); a decision that would take longer is left to a later turn's end. A restart that cannot start now (its subscription at its limit, a sandbox that cannot be joined) says nothing, is no switch decision, keeps no subscription switch waiting (it has its own ten-minute wait) and is tried at a later turn's end. A version a session could not continue on gets no other running session, and the card names the task and what happened: the restart could not be launched, the restarted session ended by itself before it started (its SessionStart hook), or it did not start within two minutes (that session is left as it is, for its person). A newer version, or a restart of the server, lifts it. A task that is idle keeps its version until its next turn ends or its next start, and so does a run without hooks. The Superagent, shells and CI jobs get the newer version when they next start, as before. A restart for a newer Claude Code is no subscription switch in the task's info. |

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
- [x] Independent review round 2 answered, every finding fixed with its test
- [x] A session resumed with nothing typed names its Claude Code with its first record

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
  refused switch and a version a session could not continue on keep it where it is. A restarted
  session that ends by itself before it started, or does not start in time, stops further
  restarts on that version and the card says which task; one that started in time stops nothing.
  A restart refused before it began is silent, records no switch and leaves the subscription
  switch's own wait untouched.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: the channel default (AC2); which channel falls
  forward to `latest`; where a model id ends; the gate (default model) and the reported gap; the
  problem's and the fix's words per channel; when a session moves (`moves_now`: every refusal of
  AC9). `crates/yulius-domain/src/harness.rs`: the offered list starts with the default.
- Unit, `crates/yulius/src/wiring/claude_models.rs`: the streaming model scan (ids across chunk
  boundaries, at the end of the file, followed by a digit, a letter or a hyphen, absent) and its
  per-file memory. `crates/yulius-harness/tests/observe.rs`: the run's CLI version is the latest
  one a record of its own thread names (a resume's meta record, a tool result, an attachment,
  an answer), never a sub-agent's. `crates/yulius/src/wiring/harness.rs`: a run resumed with
  nothing typed names its version at its first record, never the earlier run's, and a recount
  says the same, from its window's lines only. `tests/observe.rs`: a version of megabytes is
  kept as 64 bytes, whichever record names it. `crates/yulius/tests/metrics_recount.rs`: a
  recount records the version its runs' lines name.
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
  when it starts): AC8; AC9 in prompt mode and in goal mode; the typing refusal; background work
  and a `yulius wait` (processes that read a FIFO until the test lets them go); a restart that
  cannot be launched, a restarted session that exits at once, one that never starts and one that
  starts in time (the fake agent's `--exit-at-start-file` and `--stuck-at-start-file`); a refused
  restart (the subscription at its limit while the turn is held by `--turn-hold-file`); and the
  idle state after a person's move of an idle task (the defect met on the way).
- e2e, `e2e/tests/claude-code.e2e.ts` (mock): AC7, AC8's task info rows, and a channel change
  and a check the server refuses.
- Not testable in CI, done by hand (see Evidence): a real download from npm with the real
  340 MB binaries, a real isolated server on a real host with the incident's Claude Code (the
  doctor, the card, the first check, the channel, "Check now"), and what real sessions resumed
  on a newer Claude Code write. Not testable here: the move of a real Claude Code session by
  Yulius (the fake agent stands in for it), including what a real release that cannot resume
  a session looks like (the fake agent exits or stays silent at its start).

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
- **Review round 2** (independent reviewer, fresh session, at aadf7b98: request changes; two
  majors, four minors and nits, all own code, all answered, each fix with a test that fails
  without it where a test can show it):
  1. (major) The fix trusted a remembered `latest`. With Off, or after a check that failed, the
     doctor and the card said "No Claude Code release knows it yet (X, the newest, does not)"
     from a value read hours or days before, and left out the host's command that would help.
     It is said now only from the server's own read of the registry in the last two hours, on a
     channel that reads it (`agent_cli::newest_lacking_default`, one rule for the doctor and the
     card): `a_remembered_latest_is_not_called_the_newest_release`, the admin view test.
  2. (major) The breaker saw only a launch that failed. A release that starts and exits, or
     opens on something nobody answers, would have taken every running task, one turn end at a
     time. A restart is watched until its session's SessionStart hook: its end by itself before
     that, or two minutes without it (`AppConfig::claude_code_start_ms`), stops further restarts
     on that version and the card names the task. A session that never starts is left as it is:
     ending it would lose what a person could still read or answer there.
     `a_restarted_session_that_exits_at_once_stops_further_restarts`,
     `a_restarted_session_that_never_starts_stops_further_restarts`,
     `a_restarted_session_that_starts_in_time_stops_nothing`.
  3. (minor) After a refused channel change the select stayed off on the refused value: the card
     redraws only on a change, and the server's answer read as before. It is drawn again from
     that answer, and so is the button after a refused check (e2e "a channel change or a check
     the server refuses…", the mock's `claude_channel_fail=1`).
  4. (minor) A refused restart, silent, stamped the subscription switch's cooldown, so a switch
     at a limit waited ten minutes for it. It has its own (`claude_move_refused_ms`):
     `a_refused_restart_is_silent_and_keeps_no_subscription_switch_waiting`.
  5. (minor) The decision could outlast the hook client's 5 s (two waits for the transcript, two
     looks at the processes): one of each, inside a budget of 3.5 s
     (`AppConfig::claude_code_decision_ms`), after which the turn's end leaves the session where
     it is: `a_decision_that_takes_too_long_leaves_the_session_where_it_is` (no time at all).
  6. (minor) Tests on timers: the goal test holds its turn on a file until the goal was read
     (`--turn-hold-file`, `App::goal_is_open`), the background shell reads a FIFO instead of
     sleeping 25 s, and the one notice is read once `ClaudeMoveSettled` was reached.
  Found while answering it, the same kind as 1: the card said "Checked … ago: npm has latest X"
  after a check that failed and with updates off, from an earlier read. A failed check says "npm
  had … when it was last read", and Off says only that nothing is checked (e2e).
  Nits: the loop counts the requests (a watch channel), so one made while a check runs is
  answered by one more check and nothing is left that starts another
  (`requests_made_while_a_check_runs_are_answered_by_one_more_check`); `ClaudeMoveProbed`'s
  words; what a session was sent to and which version is stopped are read again after the looks
  that take seconds; task info says "or at its next start"; the operations guide names the limit
  of a session's own `/loop` schedule; the last Plan step is named for what is true before a
  verdict. Left as it is: `version_for_new_runs` reads a link and the host's small manifest on
  each task detail and turn end, as every sandbox join does; a cache would have to notice the
  host's own `npm install`, which only those reads show.
- **After the merge, checked against real artifacts (2026-11-11), which found one defect.**
  PR #1167 merged at e9e11c0a. What its Evidence named as untested was then checked with what
  this sandbox holds: npm's real registry, a real host booted in it, and the transcripts of
  real sessions that were restarted on newer Claude Code releases that day.
  - *The registry and a real host.* The production updater against registry.npmjs.org
    replayed the incident with the real releases, and a real isolated server on an Ubuntu
    26.04 guest with npm's 2.1.311 went through it from the doctor's `FAIL` to the release it
    installed by itself; neither found anything to fix (Evidence).
  - *The transcripts.* Seven real sessions name two or three Claude Code versions in one
    transcript (2.1.292, 2.1.311, 2.1.322): a resumed process writes into the session's file
    and every record of the session's thread carries its version. The observer took the
    version from prompt records only. A session resumed with nothing typed (it redoes its
    interrupted turn by itself) writes a meta record, attachments and answers, and no prompt
    record until someone or something prompts it: in one real session the first prompt record
    on 2.1.322 came 3 h 2 min and 1,950 lines after the restart. For that time its run named no
    version, so task info showed the previous session's (2.1.311 for a session on 2.1.322) and
    its Sessions row none. Now every record of the session's own thread names the version
    (`claude::observe_versioned`, one parse per line as before), for the watcher and the
    recount; over those seven transcripts each of the nine restarts is seen at the first
    record of the resumed process that names a version (in three of them it first wrote two
    `queue-operation` records, which name none).
  - *The restart's own word.* A session Yulius restarts is taken to run the version it was
    restarted on until its transcript names one (review round 1, finding 11). With the version
    named at the first record, that write could come second and replace the transcript's word
    (a release installed during the restart), and the watcher would not write again. It is one
    conditional statement in the store now (`assume_run_cli_version`): a version the transcript
    named stays, and one it names later replaces the assumption.
  - *An idle session resumed with nothing typed* (a person's move of an idle task to another
    subscription) writes no record until its next prompt. Its run names its version then; until
    then task info shows the last session that named one, and its Sessions row none. Only a
    restart Yulius makes for a newer Claude Code knows which version the new run gets.
  - *Its review* (independent, fresh session, at 736cc059): approved, with two minors and five
    nits in the fix's own code, all fixed here. The restart's write had no test of its order,
    and the statement none of a run whose figures were saved first: a store test of the three
    cases, and `a_restarted_session_keeps_the_version_its_transcript_named_first`, which holds
    the restart's write at the pause point `ClaudeMoveRestarted` until the transcript's version
    is recorded (the fake agent's `--resume-record` writes a meta record, as a real resume may
    before any prompt). Comments that described the old rule; a recount's window pinned for the
    version (the first run's, and a window without a line); the changes entry's first sentence,
    which promised more than the fix; three wordings of this section; a clone made before it
    was known to be needed; a blank line.
  - *Its second look* (the same reviewer, at fc6ff2f3): approved again, with one minor (the new
    restart test's last check did not wait for the turn's decision, so it could not see a
    third restart: it waits now) and a wording nit (a meta record is one of the records a real
    resume writes before any prompt, not always the first), both fixed. It also named what was
    still open beside the fix: a recount read a run's version and dropped it, so a run only a
    recount read (its live watch did not see its lines) had none. A recount records it now.
  - *Its third look* (at 3b4f5afb): approved, with one optional nit, done: the recount writes the
    version as the transcript's word, which replaces an assumed one, and a test now says so
    (the recount test assumes another version for its first run beforehand). The reviewer also
    built this change with task #452's branch on top: its tests and this change's pass.
  - *The version is cut at 64 bytes where it is kept* (`UsageAccumulator::note_cli_version`).
    Task #452 answers the third review's finding that the stored version is whatever a
    transcript line holds (a line may be megabytes) by cutting it in the parser's prompt
    records. The first-record rule reads it from every record, past that cut; both changes
    would have merged without a conflict and left the string unbounded. Cut where every path
    ends, with the parser's own rule (a character boundary, an ellipsis, a version that is no
    plain `X.Y.Z` kept), the two agree whichever merges first.
- **Migration `0050_claude_code.sql` is schema version 51.** Its number was reserved first, but
  SPEC-459's `0051_instance_limits.sql` merged first as version 50; a version is a migration's
  place in the list, so this one goes after it.
- **A shared CI blocker, fixed with the parallel task's own commit.** This pull request's first
  Yulius CI run (bb150014, at 8f5a2972) failed in `e2e` on `onboarding.e2e.ts` "a composer left
  before its reads answer adds no second toast to the start" (`expected 2 to be 1`), exactly as
  PR #1168's run had four minutes earlier with none of this change in it. The test set its hold
  on `GET /api/auth/recovery` while an earlier read could still be on its way through the mock's
  50 to 200 ms latency, so two reads reached the hold between two polls of the count. Task #432
  had fixed it in fa895b4e (PR #1151, not merged yet): the hold is set once every read asked so
  far has answered. That commit is cherry-picked here unchanged (3896abb2), so both pull requests
  carry the identical change and whichever merges second merges cleanly.
- **Not from this change:** `first_message::{a_draft_typed_during_the_opening_turn_is_left_alone,
  focus_and_mouse_reports_do_not_keep_the_goal_out,
  trust_words_after_the_session_started_are_no_trust_prompt}` and
  `merge_train::a_member_that_cannot_merge_is_told_instead_of_tested_again` fail the same way on
  the base commit 911e3127 (run there in a second worktree); issue #1154 tracks them (they fail
  in a task sandbox and pass in Yulius CI).

## Evidence

- `scripts/dev ci` at 46f40623 (review round 2's fixes and `dev` up to 4f90a309 merged): exit
  0. Repository contracts, check, fmt, clippy `-D warnings` on Linux and for macOS, nextest 3531
  passed / 16 skipped, doctests, `cargo deny` (advisories, bans, licenses, sources ok), `cargo
  shear`, doc. Earlier runs passed at a0f7ae35 (3318 tests), e22a0d8a (3325) and 457d02f7
  (3402). The commit after 46f40623 changes this file only.
- Yulius CI at 8f5a2972 (run bb150014): `e2e` 918 passed, 1 skipped, 1 failed (the onboarding
  race above; `quality` was cancelled by that failure). Reproduced here by letting the recovery
  reads asked within the same 3 s arrive together: the test and mock before the fix failed 3 of
  3 (`expected 2 to be 1`, then `3` twice), with the cherry-picked fix 3 of 3 passed, and the
  file passes with the mock as it is (14 passed).
- The incident as a test, seen failing first:
  `claude_updates::a_release_that_lacks_the_default_model_is_refused` answered
  `Ok(Installed(2.1.311))` on the base (a release without `claude-fable-5-2` became current); it
  passes with the check.
- Tests added or changed (all pass):
  - `yulius-domain` `agent_cli` (14 unit tests): the default channel, the fall-forward tag,
    where a model id ends, the gate and the reported gap, the problem and fix sentences,
    `moves_now` with each refusal, and when npm's newest release is named
    (`newest_lacking_default`: fresh, read, not off).
  - `yulius` lib `wiring::claude_models` (5): the streaming scan and its per-file memory.
    `yulius-harness` `observe`: the run's CLI version.
  - `claude_updates` (23): AC1 with the refusal remembered, AC3 in four scenarios, AC4, AC2, the
    composer's list equal to the server's, the status file and record after refused, installed,
    up-to-date and failed checks, AC5 with the unknown-version and "no release knows it"
    sentences, a remembered `latest` after a failed check, with updates off and three hours on,
    a channel change and "Check now" without a restart, requests that arrive together, requests
    made while a check runs (each read of the registry stepped by the test), and SPEC-421's
    tests.
  - `claude_code_admin` (3): AC6, the view's problem and fix (also with updates off and after an
    old read), a body without the channel.
  - `claude_code_moves` (13, real processes): AC8; AC9 in prompt and goal mode; the typing
    refusals; a background shell and a `yulius wait`; a Done goal task's follow-up; a restart
    that cannot be launched, a restarted session that exits at once, one that never starts and
    one that starts in time; a refused restart; a decision with no time; the idle state after a
    person's move.
  - `analytics`: a restart is no subscription switch.
  - e2e `claude-code.e2e.ts` (7): AC7, a refused channel change and check, and the task info
    rows, no WCAG violations; with `update`, `sandbox-docker`, `instance-limits`, `leftovers`,
    `vidoc-switch`, `disk-files` (the same page), `responsive`, `text-fits` and `onboarding`:
    141 passed.
- Mutations, each caught by a test and reverted: `stable` never falls forward; the version in use
  always counts as knowing the default; `stable` falls forward whenever its release was refused;
  a `yulius wait` not counted as background work; typing not read again after the probes; the
  breaker never set; the restarted run's version not recorded; a goal session moved at every
  Stop. Round 2: the doctor and the view trusting a read of any age; "the newest" named with
  updates off; a session that ends as it restarts, and one that never starts, stopping nothing;
  a started session not noticed; a refused restart stamping the subscription switch's wait, and
  one recorded and said like a refused switch; the decision without its time limit; a stored
  wake-up starting another check; the card not drawn again after a refused change or check;
  "npm has" after a failed check; the earlier check's line with updates off.
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
- Review: round 1 (fresh session, at beb8fca9) and round 2 (fresh session, at aadf7b98)
  requested changes, both answered above; round 3 (fresh session, at e9e11c0a) approved with
  two minors and seven nits, which task #452 fixes (SPEC-468). PR #1167 merged at that head.
- After the merge, the real registry (2026-11-11 20:37 UTC, `ClaudeUpdater::new`, a host
  stand-in of version 2.1.300, channel `stable`; a test written for the run and not kept):
  1. The host knows the default model: npm's `stable` 2.1.311 was downloaded, read and refused
     in 13.9 s ("2.1.311 does not know claude-fable-5-2 (Yulius's default model)"); nothing
     installed, nothing left but `status.json`; the doctor warns and names the refusal.
  2. The host does not know it (the incident): the doctor fails with the fix. The next check
     took `latest`'s 2.1.322 in 17.0 s without downloading 2.1.311 again; `current` points at
     it, the doctor is `ok` ("2.1.322, kept current by Yulius … (the host has 2.1.300)"), and
     the status file says `installed`, `in_use` 2.1.322, nothing missing.
  3. The next check: up to date in 0.2 s.
  4. The installed `claude.exe` is byte for byte the one `npm install -g` left on this host
     (341,914,128 bytes, sha256 `bfb3703a…4c478`, mode 755). Nothing was run.
- After the merge, real transcripts: seven sessions of this sandbox that were restarted on a
  newer Claude Code, read by the observer before and after the fix (a test written for the run
  and not kept). The first run of each names its version three to six lines earlier; the
  restarts are seen at lines 708 and 1198 instead of 739 and 3148 (session 3b93b554), 2159
  instead of 2163, 481 instead of 489, 450 and 4546 instead of 491 and 4553, 809 instead of
  810, and at the same line twice.
- Seen failing first: `wiring::harness::tests::a_run_resumed_with_nothing_typed_names_its_claude_code_at_once`
  read `None` after the resumed run's first record.
- After the merge, a real host (2026-11-11 21:07 to 21:18 UTC): an Ubuntu 26.04.1 guest (QEMU,
  kernel 7.0.0-38) with Node 22 and `npm install -g @anthropic-ai/claude-code@2.1.311`, the
  incident's host (`claude --version`: 2.1.311; its binary holds `claude-fable-5-2` 0 times
  and `claude-fable-5-1` 13 times), and a release build of 93529f5d installed with `yulius
  host setup` (isolation on, base built, service started).
  1. The doctor, run by host setup: `FAIL  Claude Code in sandboxes 2.1.311 from the host …
     Claude Code 2.1.311 does not know claude-fable-5-2, Yulius's default model … Fix: Yulius
     installs a release that knows it at its next check … `sudo npm install -g
     @anthropic-ai/claude-code@latest``; host setup ends with that item failed.
  2. The server, one second after its start (21:14:46), logs the same sentence as an error
     and writes `in_use` 2.1.311 with `in_use_missing` `claude-fable-5-2` to `status.json`.
  3. The card in the sandbox browser, signed in as the superadmin: the red banner with that
     sentence and the fix, "Sandboxes run Claude Code 2.1.311", "default model unknown", "the
     host's install", the channel Latest "From the server's configuration".
  4. By itself at 21:16:59, 2 min 14 s after the start: "Claude Code updated: new runs use it
     version=2.1.322". `current` points at `versions/2.1.322`, whose `claude.exe` has the
     sha256 of npm's own (`bfb3703a…4c478`); the doctor says `ok … 2.1.322, kept current by
     Yulius … (the host has 2.1.311)`; the card, reloaded, "knows every offered model", "kept
     current by Yulius · the host has 2.1.311", "npm has latest 2.1.322 · stable 2.1.311.
     Installed 2.1.322: new runs use it."
  5. Off chosen on the card: applied without a restart. The banner is back with "Claude Code
     updates are off: choose Latest …", "Updates are off: nothing is checked.", "Check now" is
     off, "Chosen here by superadmin just now"; the doctor says "updates are off (chosen in
     Settings, Instance)" with the same fix and exits 1.
  6. "Use the server's (Latest)": "Claude Code follows Latest now. Checking for a release…",
     then "Up to date: runs get 2.1.322." "Check now" then shows "Checking npm for a newer
     release…" and the view ends `up_to_date`, checked at 21:18:04.
- `scripts/dev ci` at 768f2c38 (that fix, on `dev` at eb2e8a4e): exit 0, nextest 3533 passed /
  16 skipped, and every other step as above; the reviewer ran it again at 736cc059 with the
  same result. Mutations, each caught by a test and reverted: the watcher ignoring a line's
  version; the recount ignoring it; a sub-agent's record naming the session's version; the
  assumed version replacing the transcript's; and, after the review, the restart writing its
  version without the condition, the statement doing nothing where a row exists, and a recount
  naming versions from lines outside its window.
- `scripts/dev ci` at f9b2b5ce (the review's answers, with `dev` up to 4b59bce3 merged): exit
  0, nextest 3639 passed / 16 skipped, and every other step as above; the reviewer ran it at
  fc6ff2f3 with the same result, and the new restart test 26 times (20 alone, 6 with its file).
- `scripts/dev ci` at f30d6c05 (the second look's answers, with `dev` up to 63819769 merged):
  exit 0, nextest 3640 passed / 16 skipped, and every other step as above. The commit after it
  changes this file only. Mutations, each caught by a test and reverted: the version kept
  whole; a recount dropping the version; and, for the restart test's last check, a restart
  whenever the versions differ (3 runs instead of 2). The reviewer ran the gate again at
  3b4f5afb (3640 passed / 16 skipped). After it: `dev` up to 143d6084 merged cleanly, and one
  line of the recount test, whose mutant (the recount writing as an assumption) fails it.
- `yulius security-scan`: "Security scan disabled: Vidoc is turned off in Yulius" (exit 0):
  disabled, not passed.
- Not tested: anything that needs a signed-in Claude subscription, which the guest has none
  of: a restart of a real Claude Code session made by Yulius (the fake agent stands in; what
  real restarted sessions write was read from their transcripts), how a real release that
  cannot resume a session fails (the fake agent exits or stays silent at its start), and so
  the card's list of tasks on an older version and a task's info against a real server (the
  mock, and the API with real processes of the fake agent); a session's own `/loop` schedule,
  which a restart ends (Yulius does not know it); a Claude Code whose binary no longer names
  its models as plain text (the check would refuse every release, which the card and the
  doctor would say).
