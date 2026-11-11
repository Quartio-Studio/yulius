---
yulius-task: 63aa8551-e6b8-4a74-bf89-2c12155c2ed0
status: draft
---

# SPEC-461: Claude Code in sandboxes always knows the models Yulius offers

Status: proposed
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
| CCM-003 | **Verified before current.** A downloaded release is checked for every offered model in the same pass as its version. One that does not know the default model is refused: nothing is installed, `current` stays, no temporary directory is left, and the refusal names the release and the model. One that knows the default model but not another offered model becomes current and the gap is reported (CCM-006). |
| CCM-004 | **`stable` never falls behind the default model.** On `stable`, when neither the version runs get nor stable's release knows the default model, `latest`'s release is taken instead, under CCM-003, and the report says so. On `latest` there is nothing newer to take: the refusal is reported. On `off` nothing is installed. |
| CCM-005 | **The version in use is checked too.** The Claude Code runs get (the newer of the host's install and Yulius's `current`) is read for the offered models when the server starts, after every check and when the channel changes (cached per file while it does not change). |
| CCM-006 | **Reported.** After every check the server records the channel, the time, npm's `latest` and `stable`, what the check did (installed, up to date, refused with the release and its missing models, or failed with the reason), the version in use, where it comes from and the offered models it does not know, in `<data>/claude-code/status.json` (for `yulius doctor`) and in memory (for the Instance page). A refusal is logged as a warning and a version in use that does not know the default model as an error. |
| CCM-007 | **Doctor.** `yulius doctor --contained` prints "Claude Code in sandboxes" as `FAIL` when the version runs get does not know the default model, naming the version, the model and the fix (what Yulius does at its next check, "Check now" in Settings, and `sudo npm install -g @anthropic-ai/claude-code@latest` on the host); as `warn` when it lacks another offered model or the last release was refused; else `ok`. It names the channel in effect: the one chosen in Settings (read from the status file) or `YULIUS_CLAUDE_UPDATES`. |
| CCM-008 | **Instance page.** Settings, Instance has a "Claude Code" card (superadmin): the channel and where it is set, the version in use and where it comes from, the host's version, npm's `latest` and `stable` at the last check, when it was checked and what it did; a danger banner with the fix while the version in use does not know the default model; a warning for another unknown offered model, a refused release and a failed check; "Check now"; the channel as a choice of Latest, Stable and Off; and the tasks that still run an older Claude Code (CCM-013). |
| CCM-009 | **Channel in Settings.** `PUT /api/admin/claude-code/channel {"channel": "latest"\|"stable"\|"off"\|null}` (superadmin) stores the channel in the database (`null`: back to `YULIUS_CLAUDE_UPDATES`). A stored channel wins over the unit's. It applies at once, without a restart: the next check runs now, runs started from then on bind as the channel says (`off`: the host's install), and it is audited `claude_code.channel`. An unknown channel is refused (422). |
| CCM-010 | **Check now.** `POST /api/admin/claude-code/check` (superadmin) starts a check at once and answers 202 with the view saying it runs; one check runs at a time (a second request joins the running one). The hourly check and the first check 2 minutes after a start stay. `GET /api/admin/claude-code` answers the view. |
| CCM-011 | **Sessions record what ran.** Each session of a task records the Claude Code version its transcript lines name, beside the model that answered (kept already). Both survive a server restart (they are read from the run's transcript lines again). |
| CCM-012 | **Task info.** A task's info shows the model it asked for (or "Default" with the default's name), the models that answered, and the Claude Code version of its latest session, with "moves to X at the end of its turn" while it runs an older one; each row of its Sessions table shows that session's Claude Code version and model. |
| CCM-013 | **A running task moves.** When a turn of a running task ends and its session runs an older Claude Code than runs get now, Yulius restarts the session on the newer one in the same Claude session (`claude --resume`, same subscription, the path a subscription move takes at a turn boundary) and says so in one line in its terminal. Not while its background shells or agents run, not while a person typed into it in the last minute, not within the cooldown of a refused switch, and once per version per task. A session that could not continue on a version stops further moves to that version (the card says so). A task that is idle keeps its version until its next turn ends or its next start. The Superagent, shells and CI jobs get the newer version when they next start, as before. |

## Plan

- [ ] Spec and ADR written, the incident reproduced as a failing test
- [ ] A release that lacks the default model is refused, and `latest` is the default channel
- [ ] The version in use is checked and reported, and the doctor fails loudly on it
- [ ] The channel is a stored setting that applies at once, with Check now
- [ ] Settings, Instance shows the Claude Code card
- [ ] Sessions record their Claude Code version and task info shows it with the models
- [ ] A running task moves to the new Claude Code at its turn boundary
- [ ] Operations guide, API reference, architecture and changes updated
- [ ] Independent review answered and every gate green

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
  (`AttemptView.cli_version`, `TaskDetail.cli_version`), after a server restart too; task info
  shows the model asked for, the models that answered and the version.
- AC9 (CCM-013). A running task whose session runs 2.1.311 while runs get 2.1.322 is restarted at
  the end of its turn: a second run of the same Claude session and subscription, launched by
  resume, with one terminal line; nothing is restarted again at the next turn end, nor while a
  background shell runs, nor when the versions are equal.

## Test plan

- Unit, `crates/yulius-domain/src/agent_cli.rs`: the channel default (AC2); which channel falls
  forward to `latest`; the refusal's and the fix's words. `crates/yulius-domain/src/harness.rs`:
  the offered list starts with the default.
- Unit, `crates/yulius/src/wiring/claude_models.rs`: the streaming model scan (ids across chunk
  boundaries, at the end of the file, followed by a digit, a letter or a hyphen, absent) and its
  per-file memory.
- Integration, `crates/yulius/tests/claude_updates.rs` (the registry stub of SPEC-421, whose
  binary stand-in now embeds the offered model ids): AC1, AC3, AC4, the status file after each
  outcome, AC5 (the doctor's level and text), the composer's list equal to the server's
  (`models.js`), the channel applied at once to binds, Check now joining a running check, and
  every SPEC-421 test unchanged in meaning.
- Integration, `crates/yulius/tests/claude_code_admin.rs`: AC6 over the app (store round trip of
  the channel, the superadmin gate, the audit entry, the view from a fake adapter) and AC8.
- Integration, `crates/yulius/tests/claude_code_moves.rs` (real processes, the fake agent with
  hooks and a transcript that names a version): AC9 and its three refusals.
- e2e, `e2e/tests/claude-code.e2e.ts` (mock): AC7; `sessions`/task info e2e for AC8's rows.
- Not testable here: a real download from npm and a real 341 MB binary in CI (the stub's stand-in
  is small); the move of a real Claude Code session (the fake agent stands in for it).

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
- **Idle sessions are not restarted in place.** A resumed goal session gets its goal typed again,
  which would start work nobody asked for in an idle task; an idle task moves at the end of its
  next turn, or at its next start.
