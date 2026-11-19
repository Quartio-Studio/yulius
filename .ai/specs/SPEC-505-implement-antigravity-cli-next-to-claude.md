---
yulius-task: f3ef7d63-9ca5-49a8-bce2-706f2f3e21c0
status: done
---

# SPEC-505: Antigravity CLI as a third harness, picked at random by default

Status: done (delivered in six slices: pull requests #1395, #1399, #1402, #1405, #1406, #1407)
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
| AGY-002 | An Antigravity run is `<agy> --model <model> --effort <effort> [--dangerously-skip-permissions] [--conversation <id>] [--prompt-interactive=<prompt>]`, model `gemini-4-carbon` and effort `high` when the task names none (amended by [SPEC-519](SPEC-519-this-is-a-test-task-confirm-everything-w.md) NEO-001: `gemini-4-neon` at `high`), with `AGY_CLI_DISABLE_AUTO_UPDATE=true`, the Yulius MCP as the `yulius mcp` stdio bridge in `config/mcp_config.json`, Yulius's hooks in `config/hooks.json`, `settings.json` written by Yulius (API key: `modelProvider: gemini`), the API key only in the secret environment, and no secret in argv or in a file other than the account's token file (0600). | `yulius-harness/tests/launch.rs` |
| AGY-003 | Each Antigravity run has its own home in its run directory, `<run>/antigravity-home`, whose `.gemini/antigravity-cli/{brain,conversations}` link to the sandbox's shared ones (`<sandbox>/antigravity/shared`, made through handles), so a conversation resumes on any Antigravity account. A contained run binds that `.gemini` (a path only the server wrote) over `<sandbox home>/.gemini`, so the agent's shell keeps its `HOME`; an uncontained run gets `HOME=<run>/antigravity-home`. | `supervisor.rs` `an_antigravity_run_binds_its_own_home`; `tests/antigravity.rs` `an_antigravity_task_runs_in_its_home` |
| AGY-004 | The transcript is observed: typed prompts (`USER_INPUT` from `USER_EXPLICIT`), model responses (tokens per step, counted once per step, keyed by its `step_index` and `created_at`, model), agent work, and a reached limit (an `ERROR_MESSAGE` with a 429/`RESOURCE_EXHAUSTED` or a plan-quota text: a limit of the subscription). Unknown types are ignored, malformed lines counted. | `yulius-harness` unit tests on a transcript real agy 1.5.10 wrote |
| AGY-005 | The session id is the `conversationId` of the run's `SessionStart` hook, else the worktree's entry in the run home's `cache/last_conversations.json` whose transcript was written after the run started. | `tests/antigravity.rs`; `wiring/harness.rs` `an_antigravity_conversation_is_found_only_when_written_since_the_run_began` |
| AGY-006 | Antigravity hooks reach Yulius as Claude Code's (`yulius hook antigravity <event>`): `SessionStart` (briefing as an injected system message, session id), the first `PreInvocation` of a turn as `UserPromptSubmit`, `Stop` (gate, holds, `continue`) and `PreToolUse` of `run_command` as `Bash` (guard). The client translates the payload and the answer; a `PreToolUse` it cannot ask Yulius about answers `allow`, so a Yulius outage never denies the agent's tools. | `yulius-harness/tests/hook_client.rs`; `tests/agent_hooks.rs` |
| AGY-007 | Yulius installs the pinned `agy` 1.5.10 (amended by [SPEC-519](SPEC-519-this-is-a-test-task-confirm-everything-w.md) NEO-003: `1.5.12`) into `<data>/antigravity-cli/1.5.10-<platform>` from the URL and sha512 in the source (Linux x86-64 and arm64), bounded, unpacked under a temporary name and renamed into place, one install at a time; runs and sign-ins start it by its absolute path, a contained run binds its directory read-only; without it a host `agy` is used, else the start fails with the reason. | `tests/antigravity_cli.rs` |
| AGY-008 | An Antigravity account signs in from Subscriptions by Google (the CLI's own sign-in in a server-owned terminal with a temporary home; the token file it writes is sealed in the vault) or with a Gemini API key; credentials are never returned, logged or audited. A run writes back a changed token file of its own account after it ends. | `tests/accounts.rs`; `tests/antigravity.rs` |
| AGY-009 | Rotation, a person's move and a removal of a subscription keep an Antigravity task on Antigravity subscriptions; with none usable it waits, its notices naming Antigravity. | `tests/antigravity.rs` |
| AGY-010 | The UI offers Antigravity: the composer's model menu has a Google group (Gemini 4 Carbon first, high effort by default; amended by [SPEC-519](SPEC-519-this-is-a-test-task-confirm-everything-w.md) NEO-004: Gemini 4 Neon first, Gemini 4 Carbon second), automations take the agent, Subscriptions adds an Antigravity subscription (Google sign-in or API key); the mock serves the same. | `e2e/tests/antigravity.e2e.ts` |
| RND-001 | A task created without a harness (or with `auto`) gets Claude Code or Antigravity, drawn uniformly at random among those two that have a usable subscription (Claude Code when neither has); `claude`, `codex` or `antigravity` given is kept, and so is the agent a model given without one implies (Antigravity for `gemini-*`, else Claude Code), as `chosen`. The task records `harness_choice` `random` or `chosen`, shown in its summary and kept with it for life. Superseded by [SPEC-514](SPEC-514-make-sure-harnesses-doesnt-change-themse.md) PIN-001 and then by [SPEC-520](SPEC-520-i-dont-know-why-but-i-dont-see-random-ha.md) RHM-001..003 (2026-11-19): draws uniformly across all added harnesses (`claude`, `codex`, `antigravity`) that have at least one non-exhausted subscription. | `tests/harness_draw.rs`; domain unit tests |
| RND-002 | The composer's agent defaults to Random and can be set to a chosen agent; an automation's agent may be Random, drawn for each run. Superseded by SPEC-514 PIN-003/004 and restored across all three harnesses by SPEC-520 RHM-005/006. | `e2e/tests/antigravity.e2e.ts`; `tests/harness_draw.rs` |
| RND-003 | The analytics export lists each task's harness, model and `harness_choice`, so the two agents can be compared on drawn tasks. | `tests/harness_draw.rs` |

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
- [x] Slice C1: review findings fixed and the slice delivered
- [x] Slice C2: Antigravity subscriptions sign in with Google or a Gemini API key
- [x] Slice C2: Antigravity limits, google_connected and moves within Antigravity subscriptions
- [x] Slice C2: review findings fixed and the slice delivered
- [x] Slice D: the UI offers Antigravity with Gemini 4 Carbon (High) by default
- [x] Slice D: SPEC-001/002/003 and the documents say Antigravity is accepted
- [x] Slice D: review findings fixed and the slice delivered
- [x] Slice E: a task without a chosen agent is drawn at random and records the draw
- [x] Slice E: the composer and automations offer Random or a chosen agent, and the export reports the draw
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

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
  sandbox), and a sign-in a person made meanwhile, or another run's refresh, is never replaced:
  a run's file counts only as a refresh of the credential its launch gave it.
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
- Review round 2 (head 3d1e91699, request changes; own code), answered:
  1. A run's file could replace a newer sign-in: the launch keeps the sealed credential it gave
     the run beside its home (`<run>/antigravity-given`, out of the bound `.gemini`); the run's
     end takes only a refresh of that, and swaps only while the account still holds it
     (`a_run_never_replaces_a_newer_antigravity_sign_in`: a person's new sign-in during the run
     stays, with the run's file refreshed or not).
  2. Tests: the supervisor's fallback branch (`an_antigravity_conversation_is_found_without_its_session_start`),
     a link at `<sandbox>/antigravity/shared` failing the start with nothing spawned or made
     through it (`a_linked_antigravity_folder_fails_the_start`), and a contained run seeing its
     home's hooks at the sandbox home's (`a_contained_antigravity_run_sees_its_home_at_the_sandbox_homes`;
     skipped here, bwrap cannot create namespaces in this sandbox).
  3. Only an Antigravity run's Stop skips the gate for its `error`
     (`agent_hooks::only_antigravity_skips_the_gate_for_a_failed_turn`).
- After round 2: `antigravity::*` 9 tests, 3 of 3 under `--stress-count 3`; `yulius-app`,
  `yulius-harness`, `yulius-contracts` 891 passed; `yulius` integration files `antigravity`,
  `codex*`, `agent_hooks`, `accounts`, `task_delete`, `supervision`, `pool_supervision` 156
  passed. Clippy clean.
- Review round 3 (head 22a92e712): approve. Its notes, fixed after: a contained plan's bind of
  the run's `.gemini` is asserted on the plan itself, where any host runs it
  (`a_contained_antigravity_plan_binds_the_runs_home`, a recording session host); an
  unreadable `antigravity-given` is logged. `antigravity::*` now 10 tests, all passed.

### Slice C2 (Antigravity subscriptions)

- `POST /api/admin/accounts` takes `provider: google`: with `api_key` (a Gemini API key) the
  account is `ok` at once, sealed as `{"kind":"gemini_api_key"}` and audited `via=api_key`
  (`accounts::an_antigravity_account_can_be_added_with_a_gemini_api_key`); without one the
  Antigravity CLI's own sign-in runs in a temporary home with `AGY_CLI_DISABLE_AUTO_UPDATE=true`,
  and the token file it writes in `.gemini/antigravity-cli` is sealed as
  `{"kind":"google_sign_in"}` (`accounts::an_antigravity_google_sign_in_seals_the_token_file`).
  `{"provider":"google"}` left the refusal list.
- `/api/me` says `instance.antigravity_connected` for a usable Google account
  (`claude_gate::a_google_account_alone_is_antigravity_connected`); the CLI installs in the
  background at start only when a Google subscription exists
  (`codex_cli::the_clis_are_installed_at_start_only_for_their_subscriptions`, `warm_agent_clis`).
- What a task says names its provider with its article (`account_gate::a_kind`: "a Claude", "a
  Codex", "an Antigravity"): a person's move to the Claude subscription is refused ("… is not an
  Antigravity subscription"), a removal moves the task to the other Google account in the same
  conversation, and with none left it waits under Needs you ("Sign an Antigravity subscription
  in again …"), its notices never naming Claude
  (`antigravity::an_antigravity_task_moves_only_between_antigravity_subscriptions`). The limit
  move was proven in C1 (`an_antigravity_task_at_its_limit_moves_to_another_google_account`).
- Tests: `antigravity::*` 11 tests, 3 of 3 under `--stress-count 3`; `yulius-app`,
  `yulius-harness`, `yulius-contracts` 892 passed; `yulius` integration files `antigravity`,
  `antigravity_cli`, `codex`, `codex_cli`, `accounts`, `account_token`, `claude_gate`, `moves`,
  `rotation`, `cockpit` 165 passed. Clippy clean.
- Mutation testing (`scripts/dev mutants`, on d005f833a): `mutants: 80 listed, 49 caught,
  0 missed, 0 timed out, 31 unviable, 0 not reached (bound 30 min, took 1081 s)`.
- Review round 1 (yulius-reviewer, head d005f833a, request changes; own code unless said),
  answered:
  1. Major, the Google arm of `holds_credential` held by no test: the real TUI never exits, so
     `accounts::an_antigravity_sign_in_ends_once_its_token_file_is_written` runs on after the
     file (the fake host's `after` output) and asserts the stop, `ok`, the seal and the wipe.
  2. Major, the provided CLI's path for a sign-in: `an_antigravity_sign_in_runs_the_provided_cli`.
     Both tests fail with their arm mutated (checked by hand).
  3. The installs before a sign-in and after an API-key account:
     `the_antigravity_cli_installs_for_a_new_google_account` (an install that fails with no host
     `agy` is 502 and leaves no account; an API key starts the install). The background install's
     match is exhaustive.
  4. The sign-in's token file is read as a run's is (`read_antigravity_token_in`: folders opened
     below the sign-in's folder without following links, `O_NOFOLLOW|O_NONBLOCK`, `fstat`,
     64 KiB): `an_antigravity_sign_in_without_its_own_token_file_fails` (no file, and the CLI
     folder made a link out of the home, both `error` with nothing sealed).
  5. "Connect an Antigravity account first" (`open_credential` uses `a_kind`).
  6. ADR-0361 and `docs/architecture.md` say the Google sign-in is the whole TUI on the host,
     stopped 5 s after its token file appears, and why that is acceptable; a sign-in that asks
     more after the file is not seen without a live account (ADR Consequences).
  7. Pre-existing, reachable from this slice: a waiting task's `waits_for_account` now follows
     every agent's accounts (`Harness::ALL`;
     `claude_gate::a_waiting_antigravity_task_is_shown_again_when_a_google_account_is_connected`).
  8. Nits: the no-update variable and the token file's folders and name are
     `yulius_domain::agent_cli::antigravity` constants used by the harness, the supervisor and
     the sign-in (`the_antigravity_token_file_is_the_domains`); the article test sits below the
     module's imports. Kept: `/api/me` asks the store once per agent (three small reads, as
     before for two). The Plan line's `google_connected` stays as worded, matching its step; the
     field is `antigravity_connected`.
- After round 1: `yulius-app`, `yulius-harness`, `yulius-contracts`, `yulius-domain` 1462
  passed; `yulius` integration files `antigravity*`, `codex*`, `accounts`, `account_token`,
  `claude_gate`, `moves`, `rotation`, `cockpit`, `subscription_moves`, `pool_supervision`,
  `supervision` 233 passed. Clippy clean.
- Review round 2 (head ee9e854a4): approve. Its notes, fixed in slice D's pull request: the
  install before a Google sign-in's account exists is held
  (`accounts::a_google_sign_in_given_up_during_the_install_leaves_no_account`, an install gate
  held while the request goes away; it fails with the install removed); a token copy that is
  not taken is zeroed; `read_login_file` is Codex's alone again; the sign-in's folders come from
  `TOKEN_DIRS`; the provided-CLI test names the release by `install_name()`.

### Slice D (the UI)

- `agents.js` names each agent (harness, provider, CLI, short name, article) for every page:
  Run on…, waiting and failed lines, the task's start screens and details, a subscription's
  usage words, the composer's Connect step, a refused start's Connect action (Connect
  Antigravity opens `?add=antigravity`).
- The composer's model menu has a Google group, Gemini 4 Carbon first (`harness: antigravity`,
  `model: gemini-4-carbon`; the server's effort default, high), kept equal to the server's
  default by `ui_contract::the_composer_offers_antigravity_on_its_default_model`. Automations
  take Antigravity as their agent. Subscriptions adds an Antigravity subscription by Google
  sign-in (the default) or a Gemini API key; its card offers neither token paste nor import.
- Login terminals are 1024 columns wide: the real CLI's Google link is 704 characters and the
  CLI breaks a link at the width (seen by the round 1 reviewer with agy 1.5.10);
  `accounts::an_antigravity_sign_in_ends_once_its_token_file_is_written` asserts the width.
- The mock serves Antigravity accounts, tasks and its Google sign-in (a 704-character link).
- Tests: e2e `antigravity.e2e.ts` 6 passed; `codex`, `subscriptions`, `claude-connect`,
  `consistency`, `model-fallback`, `automations` 98 passed (the Codex test's API-key locator
  is scoped to the shown choices, as two agents now offer one). `yulius-app`, `yulius-harness`,
  `yulius-contracts`, `yulius-domain` 1473 passed; `yulius` integration files `antigravity*`,
  `codex*`, `accounts`, `account_token`, `account_email`, `claude_gate`, `ui_contract`,
  `claude_updates` 139 passed. Clippy clean.
- The documents say Antigravity is accepted: SPEC-001 (the `F 2026-11-17 Antigravity` source,
  vision, glossary, J3, TSK-001/003, ROT-001/004, AC-ROT-8, changelog), SPEC-002 (principle 1,
  evidence table, changelog), SPEC-003 (§6 item 90, changelog), `docs/product.md` and
  `AGENTS.md`; SPEC-001's changelog, SPEC-003 item 90 and `docs/product.md` say the random draw
  follows in slice E, and the others name only what is delivered. `index.html` preloads `agents.js`
  (`check_repository.py --sync-brand`); `scripts/dev quick` passed.
- Mutation testing (`scripts/dev mutants`, on c9a973115): `mutants: 26 listed, 9 caught,
  0 missed, 0 timed out, 17 unviable, 0 not reached (bound 30 min, took 306 s)`.
- Review round 1 (yulius-reviewer, head c9a973115): approve, with minors fixed after (own code):
  an unknown `?add=` (`constructor`) opens no dialog (`Object.hasOwn`; e2e `an unknown add in
  the address opens no dialog`); Subscriptions' heading and empty state name Antigravity; e2e
  for Connect Antigravity on a refused start and the Connect Claude step leaving once an
  Antigravity account is usable, the list line's and the time tooltip's article (the three
  tests fail with the reviewer's hand mutations put back); the tests use `touchTask` and
  `firstShown` from `support.ts`; SPEC-003 item 90 defers the draw to slice E; nits: the web
  README's 1024 columns, ROT-001's comma, `defaultEffort` removed (nothing read it), the
  reserved token buffer, the C2 Plan box, the architecture diagram's `agy`. Pre-existing and
  left: the sessions page labels assistant entries "Claude" (session search and import are
  Claude Code's only, ADR-0361 Consequences). e2e `antigravity.e2e.ts` 8 passed; `yulius-app`
  611 passed; `ui_contract`, `antigravity` 22 passed; clippy clean; `scripts/dev quick` passed.

### Slice E (the draw)

- A task created without an agent (or with `auto`) takes the agent its model implies
  (`Harness::implied_by`: Antigravity for `gemini-*`, else Claude Code), and without a model is
  drawn (`App::draw_harness`): Claude Code or Antigravity uniformly among those with a usable
  subscription (`Harness::drawn` over a random byte, even over its 256 values), Claude Code when
  neither has one, whose start is then refused as before. A drawn task is recorded in
  `task_harness_draws` (migration 0066; a task absent there was chosen, as every older task
  was), and `TaskSummary.harness_choice` says `random` or `chosen`. The agents' `task_create`
  tool takes an optional `harness` with the same default.
- Tests: `harness_draw::*` (queued rolls pick each agent; `auto`; agents given or implied by a
  model are kept and say `chosen`; Codex only when chosen; the draw among usable agents only;
  neither usable refused naming Claude; an agent's `task_create` drawn unless it names one) 3
  passed; the domain's `the_draw_picks_among_the_usable_agents`; the whole workspace 4984
  passed. Clippy clean.
- An automation's agent may be `auto` (the default, stored as `auto`; `AutomationRecord.harness`
  is `None`): each run draws its task's agent and records the draw; an agent's
  `automation_create` without `harness` is `auto` too (it took the agent's own before). The
  analytics export's `harness_choice` (JSON and the CSV column after `harness`) says `random` or
  `chosen`. The composer starts with Random ("Random agent", the menu's first item: Claude Code
  or Antigravity, drawn for each task), the automation editor's Agent with Random, and a task's
  overview says "(drawn at random)"; the mock draws the same way (`harnessRolls` queues rolls).
- Tests: `harness_draw::an_automation_without_an_agent_draws_one_for_each_run`,
  `the_export_says_which_agents_were_drawn`; `automations::agents_manage_automations_through_mcp_tools`
  now expects `auto`. e2e `antigravity.e2e.ts` `a task started without choosing an agent is
  drawn, and says so`; the whole e2e suite 1095 passed (1 skipped); the whole workspace passed
  before the export change, and `harness_draw`, `analytics`, `automation` 58 passed after it.
- CI run 1627fc16 on 383f4b625 failed two tests:
  - `ui_contract::the_ui_describes_default_as_the_model_and_effort_a_launch_uses` still pinned the
    old "Default" item; own code. The Random item and the composer's chip now name each drawn
    agent's default model ("Claude Code (Opus 5.5) or Antigravity (Gemini 4 Carbon)"), and the
    test holds that wording.
  - `local_ci::a_second_run_that_finds_no_network_room_is_not_made` hit its job's 8 s limit on a
    1-CPU budget. It passes alone, and this change does not touch local CI. Filed as #1408.
- Mutation testing (`scripts/dev mutants`, on 383f4b625): `mutants: 96 listed, 15 caught,
  0 missed, 0 timed out, 81 unviable, 0 not reached (bound 30 min, took 549 s)`.
- Review round 1 (yulius-reviewer, head 383f4b625, request changes; own code), answered:
  1. Blocker: the CI failure of `ui_contract`, fixed in 880ad4a9e (above).
  2. Major: with only Antigravity usable, a mixed request on Random was refused for want of a
     Claude account, because the split card took "no `harness`" for Claude. A Random request
     while Antigravity is usable is now created as one task when the split proposal cannot be
     made. e2e `a mixed request on Random with only Antigravity usable is created as one drawn
     task` failed before the fix and passes after it.
  3. Tests added: `harness_draw::an_agent_without_a_usable_account_is_not_drawn` (relogin and
     not signed in), `deleting_a_drawn_task_removes_its_draw`, and
     `an_automation_changes_to_auto_and_back_and_templates_draw`.
  4. Spec truth: RND-001 and ADR-0361 say that a model given picks the agent as `chosen`;
     RND-002 and RND-003 name `tests/harness_draw.rs`; this mutants line; the D Plan box.
  5. `sandbox_task_create` takes `harness`. Nits: models.js comments say Random, a failed read
     of the draw is logged, the automations test's message, and api.md's default model
     (`claude-opus-5-5`, which was already stale before this slice).
  - Kept: the composer says "Random agent" also on an instance without Antigravity, where the
    draw always gives Claude Code. The label names the default the server applies.
- After round 1: the whole workspace 4989 passed. e2e `antigravity`, `codex`, `intake`,
  `claude-connect` 48 passed. Clippy clean; `scripts/dev quick` passed.
- Review round 2 (head eed6f74bb, request changes; own code), answered:
  1. Major: `sandbox_task_create` listed `harness` in its schema, but its checked arguments
     (`CrossTaskArgs`, `deny_unknown_fields`) refused it. It now takes `harness`, the request
     shows it among its options ("Agent: …", `OPTIONS`, `agent.js` `OPTION_LABELS`), and the
     approved task runs it as `chosen`. `cross_sandbox::a_request_names_the_agent_of_its_task`
     failed before the fix and passes after it.
  2. Nits: ADR-0361's edited line is wrapped. The round 1 fix's Rust product lines are a JSON
     schema literal and a log call; mutation tooling finds nothing to change in either, so the
     mutants run on 383f4b625 stands for the slice.
- After round 2: `cross_sandbox` 36 passed, `harness_draw` 8 passed.
- Review round 3 (head e5c934913): approve. It also checked the whole task and found every
  requirement, AGY-001..010 and RND-001..003, with code and a test. Its notes, fixed after:
  e2e `superagent-requests.e2e.ts` checks that the card shows "Agent: antigravity" and that the
  approved task runs it (it failed with the label line removed, per the reviewer's probe); the
  mock's approval honours the request's agent. Kept: an unknown agent name is refused only after
  the person approves, as an unknown `mode` already is. `superagent-requests.e2e.ts` 19 passed.
- Not verified without a live Google account (ADR-0361 Consequences): a real Google sign-in and
  its token file, refresh and write-back, a plan's real quota texts and reset times. Antigravity
  subscriptions show no usage windows. Claude-only features stay Claude-only for Antigravity
  tasks.
