---
yulius-task: f3ef7d63-9ca5-49a8-bce2-706f2f3e21c0
status: in-progress
---

# SPEC-505: Antigravity CLI as a third harness, picked at random by default

Status: in-progress (delivered in six slices)
Date: 2026-11-17
Owner: Yulius task #487
Scope authorization: the person's request of 2026-11-17 (task #487), verbatim: "Implement
antigravity cli next to claude cli. it should work very similiar to claude, it should be
randomised by default what we are using but also we should be able to choose. Use Gemini 4 Carbon
high by default. Also adding subcribtions, switching, everything. later we'll use transcripts and
other data to analyse which one is better and we'll stick to one." That is the decision
`F 2026-11-17 Antigravity`, which slice D records in SPEC-001 beside `F 2026-11-16 Codex`.
ADR: [0361](../../docs/decisions/0361-antigravity-is-a-third-harness-picked-at-random-by-default.md).

## Goal

A person can connect Antigravity subscriptions (a Google sign-in shown in the UI's terminal, or a
Gemini API key), and every new task that names no agent runs Claude Code or Antigravity, drawn at
random among the agents with a usable subscription; a person can still choose the agent. An
Antigravity task runs the native `agy` TUI in its terminal on Gemini 4 Carbon at high effort
unless another model is chosen. Yulius reads its transcript for turns, tokens and limits, answers
its hooks as it answers Claude Code's, moves it only between Antigravity subscriptions (at a
limit, by a person's choice, when one is removed), resumes the same conversation after a restart
or a move, and provides the pinned `agy` itself. Each task records whether its agent was drawn or
chosen, so the transcripts and outcomes of the two agents can be compared later without the
selection bias of people picking one for certain work.

## Context

- **Two harnesses today.** Claude Code, and Codex since SPEC-503 (ADR-0355): `Harness::Codex`
  with `Provider::OpenAi`, a home per (sandbox, account), a pinned CLI Yulius downloads and checks
  by sha512, hooks forwarded by `yulius hook codex <event>`, rotation and moves kept within the
  task's provider. Antigravity follows the same seams; nothing of Codex changes.
- **The Antigravity CLI (`agy`).** Google's agent CLI, the successor of Gemini CLI, a single Go
  binary. Checked by hand on 2026-11-17 against release **1.5.10** (built 2026-11-17, the newest
  on the release manifests `antigravity-cli-auto-updater-…run.app/manifests/<platform>.json`; the
  tarball `cli_linux_x64.tar.gz` holds one file, `antigravity`, 267 MB, sha512
  `713ae840…34378f`), run against a local stub of the Gemini API (`GEMINI_API_KEY` with
  `GOOGLE_GEMINI_BASE_URL`, settings `modelProvider: gemini`). Observed:
  - **Flags.** `--model`, `--effort low|medium|high|xhigh|max`, `--conversation <id>` (resume),
    `-c`, `-i/--prompt-interactive` (a prompt, then the TUI), `-p` (headless),
    `--dangerously-skip-permissions`, `--settings` (session-only settings). `--model gemini-4-carbon
    --effort high` runs `gemini-4-carbon-high`, shown as "Gemini 4 Carbon (High)"; without
    `--model` a resumed conversation fell back to the account's default (`gemini-3.1-pro-low`), so
    every launch passes the model.
  - **Files.** Everything lives under `$HOME/.gemini` (no variable moves it;
    `ANTIGRAVITY_CONFIG_REL_DIR` had no effect): `antigravity-cli/settings.json`,
    `config/hooks.json`, `config/mcp_config.json`, `antigravity-cli/brain/<conversation>/
    .system_generated/logs/transcript.jsonl`, `antigravity-cli/conversations/<id>.db`,
    `antigravity-cli/cache/last_conversations.json` (worktree → last conversation id). Without a
    D-Bus session bus the Google sign-in is kept in a file,
    `antigravity-cli/antigravity-oauth-token` (seen with `strace`), not in a keyring. A resume
    from another home works when `brain/` and `conversations/` are shared.
  - **Transcript.** One JSON object per step: `step_index`, `source` (`USER_EXPLICIT`, `MODEL`,
    `SYSTEM`), `type` (`USER_INPUT`, `PLANNER_RESPONSE`, `SYSTEM_MESSAGE`, `ERROR_MESSAGE`,
    `GENERIC`, …), `status`, `created_at`, and on a model response `model`, `input_tokens`
    (without cached tokens), `cache_read_tokens`, `output_tokens` (with thinking), `tool_calls`.
  - **Hooks.** `config/hooks.json` maps a hook name to events `SessionStart`, `PreInvocation`,
    `PostInvocation`, `PreToolUse`/`PostToolUse` (with a `matcher` on tool names such as
    `run_command`) and `Stop`; each runs `sh -c <command>` with a camelCase JSON payload on stdin
    (`conversationId`, `transcriptPath`, `cwd`, `modelName`, `lastUserInput`, `hookEventName`;
    `Stop` adds `terminationReason`, `continuationActive`; `PreToolUse` adds `toolCall {id, name,
    args}`). **A `PreToolUse` that answers `{}` denies the tool** ("tool call denied by pre-tool
    hook"): an answer must say `{"decision":"allow"}`. `Stop` answers `{"decision":"continue",
    "reason"}` to go on; `SessionStart` answers `injectSteps` with a `systemMessage`.
    `PreInvocation` has `invocationNum` 0 at the first model call of each turn and 1 after a
    continued Stop. Probed by the review of slice A: a resume (`--conversation <id>`, with or
    without a prompt) fires no `SessionStart`; when a background command ends the CLI opens a
    turn by itself (`invocationNum` 0, no new `USER_INPUT` step, `lastUserInput` still the earlier
    prompt); after its retries of a quota the turn's `Stop` has `terminationReason` `ERROR`, and a
    `continue` answer makes it try the model again; `fullyIdle` is false while a background
    command runs; Esc ends a turn with no `Stop` and no transcript line; a hook command that exits
    non-zero (127 for a missing program) denies the tool.
  - **Limits.** A 429 `RESOURCE_EXHAUSTED` is retried, then written as an `ERROR_MESSAGE` step
    ("API error (attempt 7): Error 429, … RESOURCE_EXHAUSTED …"); the release notes name the
    plan's texts "Individual quota reached", "baseline model quota reached" and "Your AI credits
    balance is too low to continue."
  - **Updates.** The CLI updates itself in the background unless `AGY_CLI_DISABLE_AUTO_UPDATE` is
    set (a variable found in the binary).
  - **Sign-in.** Without a browser the TUI prints a Google OAuth URL whose callback page shows a
    code to paste back, like Codex's device sign-in.
- **Models.** The binary lists `gemini-4-carbon-{low,medium,high}`,
  `gemini-4-hydrogen-{low,medium,high,xhigh}`, `gemini-4-argon-{low,high}`,
  `gemini-3.8-flash-{low,medium,high}`; the release notes say Gemini 4 Carbon is offered to
  sign-ins and to `GEMINI_API_KEY` connections. Google released Gemini 4 Carbon on 2026-11-10.
- **Not verifiable without a live Google account** (none is available to this task): a real
  Google sign-in and the token file it writes, its refresh, a plan's real quota texts and reset
  times, and the `/usage` figures. Each is named under Decisions and in the ADR's Consequences.
- **Non-goals.** No translation of a session between harnesses (a task keeps its agent for life,
  `F 2026-10-08` stays in force for provider switching); no usage windows for Antigravity
  subscriptions (no verified source); Claude-only features stay Claude-only (as for Codex).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| AGY-001 | `Harness::Antigravity` (`antigravity`, "Antigravity") and `Provider::Google` (`google`) exist; tasks, automations and accounts accept them once slice C opens the paths; other names stay 422. | domain tests; `tests/accounts.rs`, `tests/automations.rs` |
| AGY-002 | An Antigravity run is `<agy> --model <model> --effort <effort> [--dangerously-skip-permissions] [--conversation <id>] [--prompt-interactive=<prompt>]`, model `gemini-4-carbon` and effort `high` when the task names none, with `AGY_CLI_DISABLE_AUTO_UPDATE=true`, the Yulius MCP as the `yulius mcp` stdio bridge in `config/mcp_config.json`, Yulius's hooks in `config/hooks.json`, `settings.json` written by Yulius (API key: `modelProvider: gemini`), the API key only in the secret environment, and no secret in argv or in a file other than the account's token file (0600). | `yulius-harness/tests/launch.rs` |
| AGY-003 | Each Antigravity run has its own home in its run directory, `<run>/antigravity-home`, whose `.gemini/antigravity-cli/{brain,conversations}` link to the sandbox's shared ones (`<sandbox>/antigravity/shared`, made through handles), so a conversation resumes on any Antigravity account. A contained run binds that `.gemini` (a path only the server wrote) over `<sandbox home>/.gemini`, so the agent's shell keeps its `HOME`; an uncontained run gets `HOME=<run>/antigravity-home`. | `supervisor.rs` `an_antigravity_run_binds_its_own_home`; `tests/antigravity.rs` `an_antigravity_task_runs_in_its_home` |
| AGY-004 | The transcript is observed: typed prompts (`USER_INPUT` from `USER_EXPLICIT`), model responses (tokens per step, counted once per step, keyed by its `step_index` and `created_at`, model), agent work, and a reached limit (an `ERROR_MESSAGE` with a 429/`RESOURCE_EXHAUSTED` or a plan-quota text: a limit of the subscription). Unknown types are ignored, malformed lines counted. | `yulius-harness` unit tests on a transcript real agy 1.5.10 wrote |
| AGY-005 | The session id is the `conversationId` of the run's `SessionStart` hook, else the worktree's entry in the run home's `cache/last_conversations.json` whose transcript was written after the run started. | `tests/antigravity.rs`; `wiring/harness.rs` `an_antigravity_conversation_is_found_only_when_written_since_the_run_began` |
| AGY-006 | Antigravity hooks reach Yulius as Claude Code's (`yulius hook antigravity <event>`): `SessionStart` (briefing as an injected system message, session id), the first `PreInvocation` of a turn as `UserPromptSubmit`, `Stop` (gate, holds, `continue`) and `PreToolUse` of `run_command` as `Bash` (guard). The client translates the payload and the answer; a `PreToolUse` it cannot ask Yulius about answers `allow`, so a Yulius outage never denies the agent's tools. | `yulius-harness/tests/hook_client.rs`; `tests/agent_hooks.rs` |
| AGY-007 | Yulius installs the pinned `agy` 1.5.10 into `<data>/antigravity-cli/1.5.10-<platform>` from the URL and sha512 in the source (Linux x86-64 and arm64), bounded, unpacked under a temporary name and renamed into place, one install at a time; runs and sign-ins start it by its absolute path, a contained run binds its directory read-only; without it a host `agy` is used, else the start fails with the reason. | `tests/antigravity_cli.rs` |
| AGY-008 | An Antigravity account signs in from Subscriptions by Google (the CLI's own sign-in in a server-owned terminal with a temporary home; the token file it writes is sealed in the vault) or with a Gemini API key; credentials are never returned, logged or audited. A run writes back a changed token file of its own account after it ends. | `tests/accounts.rs`; `tests/antigravity.rs` |
| AGY-009 | Rotation, a person's move and a removal of a subscription keep an Antigravity task on Antigravity subscriptions; with none usable it waits, its notices naming Antigravity. | `tests/antigravity.rs` |
| AGY-010 | The UI offers Antigravity: the composer's model menu has a Google group (Gemini 4 Carbon first, high effort by default), automations take the agent, Subscriptions adds an Antigravity subscription (Google sign-in or API key); the mock serves the same. | `e2e/tests/antigravity.e2e.ts` |
| RND-001 | A task created without a harness (or with `auto`) gets Claude Code or Antigravity, drawn uniformly at random among those two that have a usable subscription (Claude Code when neither has); `claude`, `codex` or `antigravity` given is kept. The task records `harness_choice` `random` or `chosen`, shown in its summary and kept with it for life. | `tests/harness_draw.rs`; domain unit tests |
| RND-002 | The composer's agent defaults to Random and can be set to a chosen agent; an automation's agent may be Random, drawn for each run. | `e2e/tests/antigravity.e2e.ts`; `tests/automations.rs` |
| RND-003 | The analytics export lists each task's harness, model and `harness_choice`, so the two agents can be compared on drawn tasks. | `tests/analytics.rs` |

## Plan

Six pull requests, each merged into `dev` on green before the next begins.

- [x] Read the product context and related work, then write and validate the spec
- [x] Slice A: the domain knows Antigravity and Google while the API still refuses them
- [x] Slice A: an Antigravity transcript is observed and counted, checked on one real agy 1.5.10 wrote
- [x] Slice A: Antigravity launch plan, home files and session paths in yulius-harness
- [x] Slice A: Antigravity hooks reach Yulius as Claude Code's through the hook client
- [x] Slice A: review findings fixed and the slice delivered
- [x] Slice B: Yulius installs the pinned agy 1.5.10 into its data directory
- [x] Slice B: review findings fixed and the slice delivered
- [x] Slice C1: an Antigravity task runs in the supervisor (homes, session id, hooks, write-back, rotation)
- [x] Slice C1: the fake agent speaks Antigravity and integration tests cover an Antigravity task
- [ ] Slice C1: review findings fixed and the slice delivered
- [ ] Slice C2: Antigravity subscriptions sign in with Google or a Gemini API key
- [ ] Slice C2: Antigravity limits, google_connected and moves within Antigravity subscriptions
- [ ] Slice C2: review findings fixed and the slice delivered
- [ ] Slice D: the UI offers Antigravity with Gemini 4 Carbon (High) by default
- [ ] Slice D: SPEC-001/002/003 and the documents say Antigravity is accepted
- [ ] Slice D: review findings fixed and the slice delivered
- [ ] Slice E: a task without a chosen agent is drawn at random and records the draw
- [ ] Slice E: the composer and automations offer Random or a chosen agent, and the export reports the draw
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** (AGY-001..006) `POST /api/sandboxes/{id}/tasks` with `harness: antigravity` creates a
  task that runs the planned `agy` command in its terminal (fake agent in tests), on
  `gemini-4-carbon` at `high` when no model is given; its transcript drives turns and tokens; its
  Stop hook settles the turn; a `PreToolUse` answered by nobody still lets the tool run.
- **AC-2** (AGY-004, AGY-009) A limit step in the transcript moves the task to another
  Antigravity subscription by `--conversation <id>`, or makes it wait when none is usable; it is
  never moved to a Claude or Codex subscription, through the API either (422 naming why).
- **AC-3** (AGY-003, AGY-005) A restart reattaches the run; a later Continue resumes the same
  conversation, also on another Antigravity account.
- **AC-4** (AGY-007) On a host without `agy`, an Antigravity task starts
  `<data>/antigravity-cli/1.5.10-linux-x64/antigravity`; a tarball of another digest is refused
  and nothing is installed.
- **AC-5** (AGY-008) An administrator adds an Antigravity subscription by API key or Google
  sign-in; the credential is sealed and never shown; a run's refreshed token file is written back.
- **AC-6** (RND-001..003) Without a harness, over many tasks with both agents usable, both are
  drawn (a seeded draw in tests picks each); with only Claude usable every task is Claude; the
  task says `harness_choice: random`; a chosen harness says `chosen`; the export lists both.

## Test plan

- Unit (`yulius-domain`, `yulius-harness`): names and parsing; the transcript observer on a
  transcript the real agy 1.5.10 wrote against a stub (synthetic content, no account) and on
  synthetic limit lines; the launch plan (argv, env, files, modes, no secret in argv); the hook
  client's translation both ways and its fail-open `allow`; the draw with a seeded source.
- Integration (`crates/yulius/tests`, fake agent with `--transcript-kind antigravity`): an
  Antigravity task's launch, transcript watch, hooks, rotation at a limit, resume on another
  account, token write-back; accounts with provider `google` (API key, refusals, audit); moves
  within the provider; the installer against a local stub server (install once, concurrent
  callers, refused digest, leftovers removed); the draw at task creation and in automations.
- e2e (`e2e/tests/antigravity.e2e.ts`): Add subscription for Antigravity (API key, Google
  sign-in terminal), the composer's Random default and Antigravity choice, an automation's agent.
- Real CLI by hand: agy 1.5.10 run against a local Gemini API stub with the files Yulius writes
  (hooks called, MCP bridge listed, transcript read by the observer). No test calls Google; the
  real installer test is `--ignored`.

## Decisions

- **Third harness, same seams as Codex** (ADR-0361). `Harness::Antigravity` with
  `Provider::Google`; no translation between harnesses, a task keeps its agent for life.
  Rejected: running Antigravity through Gemini CLI (`@google/gemini-cli`, ended for subscriptions
  on 2026-06-18) or through `agy acp` (no native terminal to watch and type into).
- **Random by default among Claude Code and Antigravity**, the two agents the person wants to
  compare; Codex runs only when chosen. Uniform over the agents that have a usable subscription,
  so an instance without Antigravity behaves as before. Recorded as `harness_choice` so a later
  comparison can use drawn tasks only. Rejected: alternating (predictable, correlates with time
  of day); weighting by capacity (biases the comparison); drawing at each run (a task keeps its
  agent for life).
- **Gemini 4 Carbon at high effort** is the default of an Antigravity task, by the person's
  request; passed on every launch, since `agy` otherwise picks the account's saved model.
- **The home is `$HOME`, one per run.** `agy` reads only `$HOME/.gemini`; each run has its own
  home in its run directory (the review of slice C1: a home in the sandbox tree could be swapped
  for a link by another run of the sandbox before bwrap binds it), and a contained run binds its
  `.gemini` over the sandbox home's, so the agent's tools keep the sandbox home (git, gh, ssh).
  The conversations live in the sandbox, shared by every run. An uncontained run (development
  hosts without bubblewrap) sets `HOME` to its Antigravity home instead: its shell then sees
  that `HOME` (a known limit of uncontained runs).
- **Hook translation in the client** (inside the sandbox): the server keeps one Claude-shaped hook
  contract; the client maps Antigravity's events and answers. Its fail-open answer for
  `PreToolUse` is `allow`, because `agy` denies a tool on an empty answer, and the hook command
  falls back to printing `allow` when the client cannot run at all.
- **Which model call is a prompt.** Only the first model call of a turn whose step before it is a
  person's `USER_INPUT` (read from the transcript's end) reaches Yulius as `UserPromptSubmit`;
  the turn the CLI opens itself after a background command is the agent's work, as Claude Code's
  wake-ups are, so it neither undoes an achieved goal nor counts as a prompt.
- **Which Stop is a turn's end.** A Stop after the model API failed (`terminationReason`
  `ERROR`) or after a prompt Yulius refused (`TERMINAL_CUSTOM_HOOK`) asks Yulius nothing, so no
  answer continues it (Claude Code runs no Stop hook for either). The transcript's error step
  ends such a turn as an interrupt does: a limit's moves the task, any other leaves it waiting
  (slice C1). A refused prompt's `USER_INPUT` step stays in the conversation, as agy wrote it
  before the hook ran. A Stop with
  `fullyIdle` false is a turn's end like Claude Code's with background agents still running: the
  CLI's own wake-up later is agent work.
- **A resume gets no new briefing.** The CLI fires no `SessionStart` on `--conversation`; the
  conversation keeps the briefing its start injected, and Yulius knows the session it resumed.
- **MCP servers with a bearer token are left out** of Antigravity runs (the CLI expands no
  variable in a header, and the token would be written to a file); slice C1's run notice names
  the servers left out.
- **Limits.** A plan's quota text, or a 429 naming the quota it used up (`quotaId`, not a
  per-minute one), is a limit of the subscription at its first attempt (the CLI may try a few
  times more, each attempt the same limit). A 429 naming no quota is the model's capacity,
  which the CLI retries.
- **For slice C1** (from the review of slice A): a step's `created_at` has whole seconds while
  the supervisor compares milliseconds, so a prompt in the same second as a Stop must not read
  as the ended turn's; an Esc ends a turn with no Stop hook and no transcript line; a refused
  prompt's `USER_INPUT` step reads as a prompt in the transcript, and no Stop follows it (the
  CLI also sends its text to its title model); an `ERROR` Stop Yulius answers with a block is
  not continued, so the server must settle that turn rather than wait for the agent.
- **The Google sign-in credential is the CLI's own token file**, captured from a temporary home
  after the CLI's sign-in, opaque to Yulius (its format is not documented, and no live account was
  available to read one); written back after a run when it changed, as the account's own file.
  Known limit: not verified live, and the file cannot be checked (unlike Codex's `auth.json`,
  whose write-back keeps only a refresh of the same sign-in): the agent of a run can replace its
  account's stored sign-in with any file it writes there, spoiling it until a person signs in
  again. Only that run's agent reaches the file (it lies in the run's own home, not in the
  sandbox), and a sign-in a person made meanwhile is never replaced.
- **No usage windows** for Antigravity subscriptions in this delivery: the only source is
  `/usage` of a signed-in CLI, whose output was not seen live. A limit is learnt from the
  transcript, with an unknown reset (the pool's default wait).
- **Pinned `agy` 1.5.10** by URL and sha512 per platform in the source, auto-update off; a bump
  is a code change. Linux x86-64 and arm64 only; other hosts use a host `agy`.

## Evidence

### Slice A (the adapter)

- **Real CLI, by hand** (agy 1.5.10 linux-x64, tarball sha512 equal to the manifest's; a local
  stub of the Gemini API, `GEMINI_API_KEY` with `GOOGLE_GEMINI_BASE_URL`, no account):
  - Transcripts `tests/fixtures/antigravity/quota-1.5.10.jsonl` (three `-p` prompts on one
    conversation, the third meeting a daily-quota 429 retried eight times) and `tool-1.5.10.jsonl`
    (a `run_command` call, a tool step a hook denied, an invalid call), paths shortened to `/w`.
  - A `PreToolUse` hook answering `{}` denies the tool ("tool call denied by pre-tool hook").
  - A hook `timeout` of 694800 s is accepted ("loaded 1 named hooks").
  - Stdio MCP servers inherit the CLI's environment (`YULIUS_URL`, `YULIUS_TASK_TOKEN` seen by a
    server that printed its environment); `${VAR}` in an HTTP server's `headers` is sent
    verbatim, so a bearer-token server is left out of Antigravity runs.
  - `AGY_CLI_DISABLE_AUTO_UPDATE=true` stops the updater ("Auto-update disabled via environment
    variable"); `1`, `TRUE` and `yes` do not.
  - The TUI's first-run colour screen is skipped by `colorScheme` with `cache/onboarding.json`,
    the trust prompt by `trustedWorkspaces`; `--prompt-interactive=-starts with a dash` arrives
    as that prompt.
  - With the files the launch plan writes and `target/debug/yulius hook antigravity <event>`
    against a fake Yulius: the SessionStart briefing is injected (its text in the transcript),
    UserPromptSubmit carries the prompt, `run_command` runs, a blocked Stop continues with the
    reason as a system message and the next Stop ends the turn; the `yulius mcp` bridge reached
    the fake Yulius's `/mcp`.
  - A conversation resumes from another `HOME` whose `brain/` and `conversations/` link to the
    first's.
  - Without a browser the TUI prints a Google OAuth URL (callback
    `antigravity.google/oauth-callback`); `strace` shows the token file
    `.gemini/antigravity-cli/antigravity-oauth-token`. Not completed: no Google account.
- **Tests:** `cargo nextest run -p yulius-harness -p yulius-domain -p yulius-app` 1393 passed;
  `-p yulius --lib` 650 passed; the `yulius` integration files the slice touches (`task_delete`,
  `accounts`, `automations`, `codex`, `agent_hooks`, `account_usage`) 103 passed.
  `cargo clippy --workspace --all-targets -D warnings` clean.
- **Mutation testing** (`scripts/dev mutants`, on 2dfbf0ed5): `mutants: 231 listed, 60 caught,
  0 missed, 0 timed out, 56 unviable, 115 not reached (bound 30 min, took 1802 s)`.
- **Review round 1** (yulius-reviewer, head cba71f5bf, request changes; no blocker, nothing
  pre-existing), each answered in 2dfbf0ed5 and the commit after it:
  1. A wake-up after a background command was sent as the last prompt: fixed test-first
     (`an_antigravity_wake_up_is_no_prompt`), the step before the call must be a typed
     `USER_INPUT`.
  2. `last_conversations.json` was checked, then opened by path: fixed test-first (a linked
     folder and a FIFO in `the_last_conversation_of_a_worktree`), folders and file opened with
     `O_NOFOLLOW`, the file with `O_NONBLOCK`.
  3. A resume fires no SessionStart: recorded in Context and Decisions and the ADR.
  4. Stop's `terminationReason`, `error` and `fullyIdle`: an `ERROR` Stop is no longer sent
     (`an_antigravity_turn_ended_by_an_error_is_no_stop`); `fullyIdle` false is a turn's end,
     as decided under Decisions.
  5. A hook command that cannot run denied the tool: the command falls back to `allow`, checked
     with the real agy and a missing client (the command ran).
  6. Every attempt's error was a limit: only a quota text or a 429 naming its quota (not
     per-minute) is one now; a bare 429 is capacity (`agy_limits_and_broken_lines`).
  7. No mutation result: above.
  8. The spec's `AGY_CLI_DISABLE_AUTO_UPDATE=1` and token key: corrected.
  9. Bearer-token MCP servers left out silently: recorded under Decisions and in the ADR; slice
     C1's run notice names them.
  10. The window test: an `until` case added, its comment corrected.
- **Review round 2** (head 2f6939eb7, request changes; no blocker, nothing pre-existing):
  1. A prompt Yulius refused was continued by a Stop answer: agy ends such a turn with a Stop of
     its own (`TERMINAL_CUSTOM_HOOK`), which now asks Yulius nothing
     (`a_refused_antigravity_prompt_is_never_continued`).
  2. A failed turn that is no quota went unheard: an `ERROR` Stop reaches Yulius again but is
     never continued (`an_antigravity_turn_ended_by_an_error_is_never_continued`).
  3. The limit texts' comment and the Limits decision say the CLI may try again.
- **Review round 3** (head 944efbb58): approve. Both round-2 fixes checked, finding 1 with the
  real agy and the client built from that head (the refused prompt's model call never happened);
  two notes for slice C1 added to Decisions.
- CI run 24ec8826 on 2f6939eb7 failed on
  `merge_train::a_member_whose_own_run_fails_leaves_the_candidate`, which this change does not
  touch; it failed on #1397 too and Yulius quarantined it as flaky (issue #1398).

### Slice B (the pinned agy)

- `yulius_domain::agent_cli::antigravity` pins 1.5.10 for linux-x64 and linux-arm64 (paths and
  sha512 from Google's manifests `manifests/linux_amd64.json` and `linux_arm64.json` of
  2026-11-17); the Codex installer became `wiring/pinned_cli.rs` (`PinnedCli`, a `Release` per
  CLI), Codex's behaviour and error messages unchanged (its log lines name the CLI in a field
  now).
- Tests: `antigravity_cli::*` (install once for concurrent callers, refused digest, missing
  binary, wrong machine, a link; the adapter's provided CLI, host `agy` fallback and reasons) and
  `codex_cli::*`, `codex::*`: 14 and 31 passed; the domain's
  `the_pinned_antigravity_cli_names_its_tarballs` passed.
- `antigravity_cli::the_real_pinned_release_installs_from_google` (`--run-ignored only`):
  passed in 8.5 s; the installed binary prints `1.5.10`.
- Mutation testing (`scripts/dev mutants`, on c5bacd7de): `mutants: 72 listed, 44 caught,
  0 missed, 0 timed out, 28 unviable, 0 not reached (bound 30 min, took 612 s)`.
- Review round 1 (yulius-reviewer, head c5bacd7de): approve; its minors and nits fixed in
  37f260c4d: a host move leaves `antigravity-cli` out (`LEFT_OUT`, walk test), tarballs naming
  `../escape` or an absolute path are refused with nothing left behind, "an Antigravity start",
  the module doc and the by-hand test's `AGY_CLI_DISABLE_AUTO_UPDATE=true`. The CI security job
  did not run (Vidoc refused the key, exit 78): unrun, not passed.

### Slice C1 (Antigravity tasks)

- `harness: antigravity` is accepted for tasks and automations; a model that is not `gemini-*`
  is refused (422), and a task without a Google account says "no Antigravity account is
  connected" (409) (`task_delete::tasks_run_claude_code_or_codex`).
- The supervisor makes `<sandbox>/antigravity/<account>` through folder handles, its
  `antigravity-cli/{brain,conversations}` linking (absolutely) to `<sandbox>/antigravity/shared`,
  writes the plan's files there through handles, provides the CLI (installed before the launch
  lock, its folder bound read-only), binds the account's `.gemini` over the sandbox home's in a
  contained run or sets `HOME` in an uncontained one, gives a goal task its request as its first
  message, records `gemini-4-carbon` as the run's model when the task names none, learns the
  conversation from SessionStart (else from `last_conversations.json` written since the run
  began), writes back a changed token file and removes it once no run of the account is left,
  and says which token-needing MCP servers a run goes without.
- From the slice A review's notes: a non-limit error step ends the turn as an interrupt does (a
  refused prompt, a model API failure after the retries), and an `ERROR` Stop asks Yulius
  nothing again (`an_agy_error_that_is_no_limit_ends_the_turn`,
  `an_antigravity_turn_ended_by_an_error_is_never_continued`). Steps' whole-second `created_at`
  needs no change: a typed prompt reaches Yulius through its prompt hook, which counts.
- The fake agent speaks Antigravity (`--transcript-kind antigravity`): numbered steps, the
  typed prompt's step before its prompt hook, hooks sent and answered through
  `yulius hook antigravity` in the CLI's shapes.
- Tests: `antigravity::*` (runs in its home with its conversation from SessionStart; moves to
  another Google account at a quota by resuming the conversation; a goal task's first message;
  a refreshed sign-in kept and removed; a failed turn leaves the task waiting): 5 passed, and
  5 of 5 times under `--stress-count 5`. `yulius-app`, `yulius-harness`, `yulius-domain` and
  `fake-agent`: 1438 passed. The `yulius` integration files `antigravity`, `codex*`,
  `agent_hooks`, `accounts`, `task_delete`, `automations`, `supervision`, `pool_supervision` and
  `first_message`: 186 passed. Clippy clean.
- Mutation testing (`scripts/dev mutants`, on 46771a073): `mutants: 153 listed, 73 caught,
  0 missed, 0 timed out, 80 unviable, 0 not reached (bound 30 min, took 1327 s)`.
- Review round 1 (yulius-reviewer, head 46771a073, request changes; all own code), answered:
  1. Blocker, the bind source in the sandbox tree: each run's home is now
     `<run>/antigravity-home` in the server's run directory (`an_antigravity_run_binds_its_own_home`;
     `an_antigravity_task_runs_in_its_home` asserts the home is not below the sandbox).
  2. A goal task got `/goal` typed: `follows_goal` is Claude Code's only; the goal test asserts no
     goal markers and Gemini as the shown model.
  3. Each attempt's error step ended the turn: only the refusal step (`SYSTEM_SDK`) ends one; a
     turn its model API failed ends with its `ERROR` Stop, sent with `error`, which the server
     settles without a gate or nudge (`HookInput.error`; `agent_hooks::a_stop_of_a_failed_turn_is_never_blocked`,
     `only_a_refused_agy_prompt_ends_the_turn_by_itself`, `an_antigravity_turn_ended_by_an_error_is_never_continued`,
     `a_failed_antigravity_turn_leaves_its_task_waiting` through the fake's `/apierror`).
  4. Token write-back: per run now, so no launch overwrites another run's refresh and nothing is
     shared; a launch that ends before live removes its file (`AntigravityToken`); the file is
     removed only once the account holds it (stored or unchanged).
  5. Tests added: the AGY-005 fallback and its time guard, the token read (link, FIFO, oversize,
     linked folder), the bind, the uncontained `HOME` (its length in the fake's report).
  6. The task view shows `gemini-4-carbon` at `high` for an Antigravity task without a model.
  7. `docs/api.md` names `antigravity`. 8. Gone with the per-run home (links made anew each run).
  9. The spec and ADR say what the opaque token file allows. 10. A limit is read only from
  `SYSTEM` steps. 11. The automation tool takes `antigravity`. 12–13. Fixed (`first_message_only`,
  `model_of_its_harness`). 14. The two places point at each other.
- After round 1: `yulius-app`, `yulius-harness`, `yulius-domain`, `yulius-contracts`, `fake-agent`
  1494 passed; `yulius --lib` 651; the `yulius` integration files above with `mcp*` 197.
