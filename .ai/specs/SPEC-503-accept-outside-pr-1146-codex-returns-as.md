---
yulius-task: d068a269-9b2e-4341-9686-330714daae2f
status: in-progress
---

# SPEC-503: Codex returns as a second harness

Status: in-progress (accepted 2026-11-16, delivered in five slices)
Date: 2026-11-10 (proposed as SPEC-451), 2026-11-16 (accepted, brought onto current dev)
Owner: Yulius task #485, from outside pull request #1146 by studenciak00owl (written as SPEC-451
with ADR-0290 in their fork), accepted by MrFishPL
Scope authorization: MrFishPL accepted #1146 on 2026-11-16 (task #485: "Accepting it is the
person's decision: it reverses the Codex part of `F 2026-10-08` (provider switching and session
translation stay removed, as the PR says). So SPEC-451 and its ADR … become accepted, and
SPEC-001/002/003 change the statements the PR marked 'proposed (SPEC-451)' into accepted ones").
That is the founder decision `F 2026-11-16 Codex`, which slice D records in SPEC-001: it
supersedes `F 2026-10-08` for Codex only. The contributor's fork owner asked for it verbatim: "Bring back Codex (OpenAI Codex
CLI) as a second harness in this fork, working the same way Claude Code does: Codex accounts in
the subscription pool, login from the UI, a task can be created with harness Codex, the native
Codex session runs in the task terminal, observation of its transcript, usage windows, resume after
restart." ADR: [0355](../../docs/decisions/0355-codex-returns-as-a-second-harness-with-a-pinned-cli.md).

## Goal

A person can connect Codex subscriptions (ChatGPT sign-in by device code shown in the UI, an
`auth.json` of a ChatGPT sign-in made elsewhere, or an OpenAI API key), create a task or an
automation that runs Codex, and watch the native Codex TUI in the task terminal. Yulius reads the
Codex rollout for turns, usage and rate limits, shows the subscriptions' usage windows, moves a
Codex task only between Codex subscriptions (at a limit, by a person's choice, when one is
removed), resumes the same Codex session after a restart or a move, keeps the credentials Codex
refreshes during a run, and provides the pinned Codex CLI itself, so a Codex task never fails with
`codex: not found`. A task never changes harness.

## Context

- **What was removed, and why.** Commit `9f88975a` (2026-10-08) removed the Codex adapter
  (`yulius-harness/src/codex.rs`: launch plan, `CODEX_HOME` homes, `hooks.json`, rollout
  observation and discovery, the `--no-daemon` probe), Codex login and `auth.json` write-back, the
  Codex usage probe, Codex session import, the transcript IR and cross-harness translation, harness
  proposals, the UI's harness choice and the fake agent's Codex transcripts. The reason was a
  product decision, not a defect: "we use only Claude; if Codex gets better, we'll support it". The
  adapter of the time targeted Codex CLI 0.160.1 and left three risks open for a live check that
  never happened (SPEC-002 R-02 refresh-token rotation across homes, R-03 hook configuration and
  trust, R-12 releases without `--no-daemon`).
- **The contribution.** #1146 (one commit, `865a459f`, +7456/−303 in 90 files) re-implemented the
  adapter on `dev` of 2026-11-10 (`91a0cec5`, SPEC-447 included) with the removed code as
  reference, checked against Codex CLI 0.167.0, and passed two rounds of an independent review in
  the contributor's fork. Since the removal the Claude path grew hook holds (ADR-17), the
  PostToolUse redaction and PreToolUse guard hooks, the status line feed, goal mode (SPEC-117),
  typed openings (SPEC-030), the transcript step-back (SPEC-034), sub-agent tails, recounts
  (SPEC-091) and session drift by hook (SPEC-281); those stay Claude-only unless a requirement
  below says otherwise.
- **What changed on `dev` since #1146** (133 commits; a cherry-pick conflicts in 30 files):
  - **Claude token paste** (#484, SPEC-502, ADR-0352): `CreateAccount` gained `token` and refuses
    unknown fields; `PUT /api/admin/accounts/{id}/token`; the Add subscription dialog chooses "Sign
    in here" or "Paste token"; every card has "Paste a new token". Codex's `api_key` and
    `auth_json` join that request, a token stays Claude-only, and the dialog chooses the agent
    first, then its way in (CDX-015).
  - **Model limits and their question** (SPEC-464, SPEC-473): a limit names its scope
    (`LimitScope`), and a refresh asks Claude whether a model's limit still holds
    (`UsageProbe::model_answer`). Codex reports limits of the subscription only, so its limit is
    `LimitScope::Account` and there is nothing to ask.
  - **The default model's placement** (SPEC-482, SPEC-493): the capacity view places the default
    task with its model family and fallback; a Codex subscription is placed as a task of any model.
  - **Language-server plugins switched off** (SPEC-485), **workflows** (SPEC-486), **Claude Code
    versions per run** (SPEC-461), **the move helper** (SPEC-470, `LEFT_OUT` has `migrate`), **one
    RFC 3339 reader** (#1296, `yulius_domain::time::parse_rfc3339`): each is fitted where the
    Codex code meets it; all of them stay Claude-only.
  - **Numbers**: SPEC-451 and ADR-0290 are other documents on `dev`, so this is SPEC-503 and
    ADR-0355. No migration.
- **The Codex CLI.** The newest release on 2026-11-16 is 0.169.0 (npm `@openai/codex`, tag
  `rust-v0.169.0`, 2026-11-13). Its help, `config.toml`, `hooks.json`, `-c` overrides,
  `auth.json` schema, rollout layout and hooks library were compared with 0.167.0 (Evidence), and
  it is what Yulius pins (CLI-001). Sandboxes copy the host system, which normally has no Codex
  CLI; Claude Code reaches runs by a read-only bind of its npm package (SPEC-421,
  `wiring/claude_cli.rs`). The Codex release ships its native build as platform packages
  (`@openai/codex@0.169.0-linux-x64`, 167 MB packed, 423 MB unpacked): `package/vendor/<triple>/`
  holds `bin/codex` (static musl), `codex-path/rg` and `codex-resources/`; the binary runs from
  that tree by itself.
- **What already fitted.** Rotation keeps candidates within the task's provider (`task_provider`
  in `yulius-domain/src/rotation.rs`, `Refusal::ProviderMismatch` for a person's choice), accounts
  carry a provider, and the composer groups models by provider.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CDX-001 | `Harness::Codex` (`codex`) and `Provider::OpenAi` (`openai`) exist; creating a task or an automation with harness `codex`, and an account with provider `openai`, is accepted; anything else stays 422. | `yulius-domain` harness tests; `tests/automations.rs`, `tests/task_delete.rs`, `tests/accounts.rs` |
| CDX-002 | A Codex run is `<codex> --no-daemon -C <worktree> -c <config>… [-m <model>] [--dangerously-bypass-approvals-and-sandbox] --dangerously-bypass-hook-trust [resume <id>] [prompt]` with `CODEX_HOME=<sandbox>/codex/<account>`, the account's `auth.json` and Yulius's `config.toml` and hooks written there (mode 0600), the Yulius MCP as the `yulius mcp` stdio bridge, the sandbox's MCP servers with their values only in the secret environment, and no secret in argv. | `yulius-harness/tests/launch.rs` |
| CDX-003 | Codex homes are per (sandbox, account) and share one `sessions` directory per sandbox, so a resumed session finds its rollout whichever account runs it. A session's rollout is found by its id; a new session's id is adopted from its SessionStart hook, else from the first rollout for the worktree created after the run started. | `tests/paths.rs`; `tests/codex.rs` |
| CDX-004 | The rollout is observed: session meta, user turns (not injected context), turn completion, token usage (per response, so a resumed run counts only its own), rate-limit windows (recorded on the account) and a reached rate limit (a usage limit of the subscription: the task rotates or waits). Unknown types are ignored and malformed lines counted. | `yulius-harness/tests/observe.rs` with synthetic fixtures `tests/fixtures/codex/*`, and a rollout the real Codex 0.169.0 wrote |
| CDX-005 | A Codex account signs in from the Subscriptions page by `codex login --device-auth` in a server-owned terminal (the device URL and code shown, credentials kept in `auth.json` of a temporary home) or by an OpenAI API key given in the UI; the credential is a validated `auth.json` sealed in the vault. | `yulius-harness/tests/login.rs`; `tests/accounts.rs` |
| CDX-006 | After a Codex run ends, the run's `auth.json` replaces the stored one only as a genuine refresh (same ChatGPT account and `id_token` subject, newer `last_refresh` not dated ahead, all three tokens present; never an API-key account or a signed-out one), so a refreshed token is kept and a file of another account or user, or dated ahead, is not (a file keeping the account's own ids with other tokens is: ADR-0355, Consequences). A newer copy another run of the account left is kept at launch; the file is removed once no run of the account is left in the sandbox, also when a launch ends before its run is live (a failed spawn, a subscription removed just after it was chosen). | `yulius-harness/tests/login.rs`; `tests/codex.rs`; `supervisor.rs` unit tests |
| CDX-007 | Codex hooks reach Yulius as Claude Code's do (`yulius hook codex <event>`): SessionStart (briefing, session id), UserPromptSubmit, Stop (turn boundary, gate, holds) and PreToolUse; a hook of another harness than the run's is refused. | `tests/agent_hooks.rs` |
| CDX-008 | A Codex account's usage windows come from `chatgpt.com/backend-api/wham/usage` (read-only) and from the rate limits in its runs' rollouts; an API-key account has no windows. Codex reports no limit of one model's family, so a refresh asks it nothing about one. | `wiring/usage.rs` tests |
| CDX-009 | Rotation picks only accounts of the task's provider; a Codex task at its limit moves to another Codex account by native resume, and waits when none has capacity. A task never switches harness. | `yulius-domain` rotation tests; `tests/codex.rs` |
| CDX-010 | The UI offers Codex: the composer's model menu has an OpenAI group (its choice sends `harness: codex`), automations show and take a harness, the Subscriptions page adds a Codex subscription (device sign-in, API key or imported `auth.json`), and the mock serves the same. | `e2e/tests/codex.e2e.ts` |
| CDX-011 | A Codex account takes an imported `auth.json` of a ChatGPT sign-in made elsewhere (`codex login` with a separate `CODEX_HOME`): at creation (`auth_json`) or for an existing account (`POST /api/admin/accounts/{id}/import`); only a full ChatGPT sign-in, sealed, never returned or logged; its refreshes are written back (CDX-006). Detailed by IMP-001..004. | `tests/accounts.rs`; `tests/codex.rs`; `e2e/tests/codex.e2e.ts` |
| CDX-012 | Codex runs and sign-ins start the pinned Codex CLI Yulius installs from npm into `<data>/codex-cli` (sha512 pinned in the source), never a `codex` the sandbox may lack. Detailed by CLI-001, CLI-002. | `tests/codex_cli.rs`; `tests/codex.rs` |
| CDX-013 | With SPEC-447, a Codex task moves only between Codex subscriptions: "Run #N on…" and "Continue #N on…" list the task's provider's subscriptions only ("No Codex subscription yet" when none); a person's choice of another provider's subscription is refused naming it ("<name> is not a Codex subscription, so it cannot take the task."), by Continue before the task starts, so it keeps resting; removing its subscription moves it to another Codex one in the same session, never to a Claude one, and with no Codex subscription usable it waits, its terminal and its Needs you naming Codex ("Sign a Codex subscription in again or add one in Subscriptions"). What the task says names its own provider, never Claude for a Codex task; a Claude task reads as before. | `tests/codex.rs` `a_codex_task_moves_only_between_codex_subscriptions`, `a_codex_start_that_loses_its_subscription_leaves_no_credentials`; `tests/subscription_moves.rs` unchanged; `e2e/tests/codex.e2e.ts` |
| CDX-014 | `GET /api/me` `instance.claude_connected` keeps meaning a usable Claude account (onboarding, the Superagent, split proposals and the Cockpit run Claude); `instance.codex_connected` says whether a Codex account is usable, and the task view's "Connect Claude" hint is left out while one is. The capacity view (Dashboard) names each subscription's provider and, with nothing available, resumes at the first reset of either pool; a Codex usage reading the endpoint refuses (403) says so in Codex's words. | `tests/claude_gate.rs` `a_codex_account_alone_is_codex_connected_not_claude_connected`; `tests/codex.rs` `the_capacity_view_names_providers_and_resumes_with_codex_too`; `wiring/usage.rs` `a_codex_usage_403_speaks_of_codex` |
| CDX-015 | Codex meets the Claude token paste (SPEC-502): `POST /api/admin/accounts` takes at most one of `token` (an `anthropic` account only), `api_key` and `auth_json` (an `openai` account only), else 422 naming why; the Add subscription dialog chooses the agent (Claude Code or Codex), then its way in ("Sign in here" or "Paste token"; "ChatGPT sign-in", "Import auth.json" or "API key"); a Claude subscription's card offers "Paste a new token", a Codex one's "Import auth.json". | `tests/codex.rs`; `tests/account_token.rs` unchanged; `e2e/tests/codex.e2e.ts`, `e2e/tests/subscriptions.e2e.ts` |
| IMP-001 | `POST /api/admin/accounts` with `{"provider":"openai","label","auth_json"}` (instance admin) creates the account `ok` at once with the file sealed as its credential; `POST /api/admin/accounts/{id}/import` `{"auth_json"}` replaces an `openai` account's credential and cancels a sign-in running for it. | `tests/accounts.rs` |
| IMP-002 | Only a ChatGPT sign-in is imported: a JSON object with `auth_mode` `chatgpt` and non-empty `tokens.id_token`, `access_token`, `refresh_token` and `account_id`; anything else, a Claude account, or both `api_key` and `auth_json` is 422. The file is never returned, logged or written to the audit (which names `auth_json` only); a malformed body is refused without echoing it. | `yulius-harness/tests/login.rs`; `tests/accounts.rs`; contracts `Debug` test |
| IMP-003 | An imported sign-in is what a Codex run's home gets, and the tokens Codex refreshes during the run are written back as for a device sign-in. | `tests/codex.rs` |
| IMP-004 | Add subscription → Codex offers "Import auth.json" (paste or file) with a note to sign in with a separate `CODEX_HOME`; a Codex subscription's card imports a new file. The mock serves both endpoints. | `e2e/tests/codex.e2e.ts` |
| CLI-001 | Yulius installs the pinned Codex CLI (`0.169.0`) from npm's registry into `<data>/codex-cli/<version>-<platform>`: the tarball's sha512 must equal the one pinned in the source, only regular files of `vendor/<triple>/` inside the release are unpacked, bounded, `bin/codex` must be an ELF for the platform, the move is atomic, one install at a time, earlier releases go once no process runs them. | `tests/codex_cli.rs` |
| CLI-002 | Every Codex run and Codex sign-in starts the provided `bin/codex` by its absolute path; a contained run binds its directory read-only. Without a provided CLI (an unsupported platform, or an install that failed while the host has `codex`), the host's `codex` is used; an install that failed with no host `codex` fails the start with the reason, never `codex: not found`. | `yulius-harness/tests/launch.rs`; `tests/codex.rs`; `tests/codex_cli.rs` `codex_runs_get_the_provided_cli_or_a_reason` |

## Plan

Five pull requests, each merged into `dev` on green before the next begins.

- [x] Slice A: SPEC-503, ADR-0355 and the decision of 2026-11-16 recorded
- [x] Slice A: the domain knows Codex and OpenAI while the API still refuses them
- [x] Slice A: a Codex rollout is observed and counted, checked on a rollout real Codex 0.169.0 wrote
- [x] Slice A: Codex launch plan, homes, session paths and sign-in helpers in yulius-harness
- [x] Slice A: review round 1 findings fixed test-first
- [x] Slice A: later review findings answered and the slice delivered
- [x] Slice B: Codex CLI 0.169.0 checked against the integration and pinned
- [x] Slice B: Yulius installs the pinned Codex CLI into its data directory
- [x] Slice B: review findings fixed and the slice delivered
- [x] Slice C1: a Codex task runs in the supervisor (homes, session id, hooks, write-back, rotation)
- [x] Slice C1: the fake agent speaks Codex and integration tests cover a Codex task
- [ ] Slice C1: review findings fixed and the slice delivered
- [ ] Slice C2: Codex subscriptions sign in by device code, API key or imported auth.json
- [ ] Slice C2: Codex usage windows, codex_connected and moves within Codex subscriptions
- [ ] Slice C2: review findings fixed and the slice delivered
- [ ] Slice D: the UI offers Codex in the composer, automations, Subscriptions and task moves
- [ ] Slice D: SPEC-001/002/003 and the documents say Codex is accepted
- [ ] Slice D: review findings fixed and the slice delivered

## Acceptance criteria

- AC-503-1: `POST /api/sandboxes/{id}/tasks` with `harness: codex` creates a Codex task that runs
  the planned `codex` command in its terminal (fake agent in tests).
- AC-503-2: a Codex run's rollout drives the task's turns, usage and limit; at a rate limit the
  task resumes the same session on another Codex account, or waits when none has capacity.
- AC-503-3: a restart reattaches the run and watches its rollout; a later Continue resumes it
  natively (`codex resume <id>`).
- AC-503-4: an administrator adds a Codex subscription from the UI and sees the device code, saves
  an API key, or imports an `auth.json`; the credential is stored sealed and never shown; an
  API-key file given as an import is refused with the reason, without echoing it.
- AC-503-5: a refreshed `auth.json` left by a run is written back to the account.
- AC-503-6: on a host without Codex, a Codex task starts
  `<data>/codex-cli/0.169.0-linux-x64/bin/codex` inside its sandbox.
- AC-503-7: a person moving a Codex task sees only Codex subscriptions; moving it to a Claude one
  through the API is refused; removing its Codex subscription moves it to another Codex one, and
  removing the last one leaves it waiting for a Codex subscription, never on Claude.
- AC-503-8: a token given for an `openai` account, or with an API key or an `auth.json`, is
  refused (422) and creates nothing; a Claude subscription's paste reads exactly as in SPEC-502.

## Test plan

- Unit: domain parsing; Codex observation, launch plan, login helpers, import validation and
  rollout paths in `yulius-harness`; the pinned release's platform data; usage parsing and the
  adapter's fallback to the host's `codex` in `yulius`.
- Real CLI: Codex 0.169.0 run with `codex exec` against a local stub of the Responses API writes a
  rollout and calls its hooks; the observer reads that rollout (session, turns, tokens, rate
  limits, usage limit) as it reads the synthetic fixtures.
- Integration (`crates/yulius/tests`, fake agent with `--transcript-kind codex`): a Codex task's
  launch, rollout watch, rotation at a limit, resume, auth write-back, hooks; accounts with
  provider `openai` (API key and import paths, refusals, audit, the token paste beside them);
  refusals for unknown harnesses; the installer against a local registry stub (install once,
  concurrent callers, refused tarballs, leftovers and earlier releases removed); a contained run
  reading the provided CLI through a real bubblewrap bind; SPEC-447 moves of a Codex task
  (CDX-013).
- e2e (`e2e/tests/codex.e2e.ts`): Add subscription for Codex (API key, import pasted and from a
  file, the card's import), the composer's Codex choice, an automation's agent, "Run on…" of a
  Codex task; `subscriptions.e2e.ts` keeps the Claude paste.
- No test calls OpenAI or npm, except `codex_cli::the_real_pinned_release_installs_from_npm`
  (`--ignored`, run by hand). Credentials and rollouts are synthetic.

## Decisions

- **Accepted by MrFishPL on 2026-11-16** (`F 2026-11-16 Codex`): the contribution proposed it as
  SPEC-451 and ADR-0290 and left SPEC-001/002/003 saying "proposed (SPEC-451)"; those statements
  now say accepted. `F 2026-10-08` stays in force for provider switching and session translation.
- **Codex only, no provider switching.** A task keeps its harness for life; rotation, a person's
  move and a removal of a subscription stay within the task's provider; there is no transcript
  translation and no cross-provider switch, so `F 2026-10-08` stays in force for ROT-005/006 and
  ADR-06.
- **Re-implemented, not reverted** (74 conflicting files in the contribution); the removed code is
  the reference. Brought onto current `dev` by a cherry-pick of the contributor's commit with the
  30 conflicts resolved, then cut into five slices that each build and leave `dev` working.
- **Five slices, in the order of what depends on what.** A: the adapter in `yulius-harness` with
  the domain's names (nothing reaches it from the API yet: task, automation and account creation
  still refuse `codex` and `openai`); B: the CLI installer; C1: a Codex task in the supervisor; C2:
  Codex subscriptions and moves; D: the UI and the documents that say it is accepted. Rejected: one
  pull request of about 3,800 product lines (beyond what one review reads soundly); a slice per
  crate (the domain's new variant must be matched in every crate that matches a harness, so a
  crate alone does not build).
- **Session id.** Codex has no `--session-id`: Yulius adopts it from the SessionStart hook first,
  with the rollout `session_meta` scan as the fallback.
- **Claude-only features stay Claude-only** for Codex tasks where Codex counterparts are not
  checked live: goal launch prompts (a Codex goal task gets its request as its first prompt), typed
  openings with images, step-back, status line, sub-agent tails, session import and search,
  language-server plugins, workflows, Claude Code versions per run, model limits and their
  question, and #1138's detection of a refused subscription: Codex's wording for a revoked ChatGPT
  sign-in is not known without a live account, so such a run fails instead of moving on (a known
  limit).
- **Write-back trust.** The run's `auth.json` is trusted only as a refresh of the same sign-in
  (`tokens.account_id` and the `id_token`'s `sub`), and removed after the account's last run only
  when it equals the vault's copy. Known limit: one account used at once in two sandboxes keeps two
  copies of its rotating refresh token, and one of them can go stale (R-02; not verified live).
- **Import is a credential like the API key**: no sign-in process, no temporary home. `account_id`
  and all three tokens are required: without `account_id` the write-back rule never takes a
  refresh, and the account would go stale at the first rotated refresh token. A separate endpoint
  `POST /api/admin/accounts/{id}/import` rather than a body on `relogin`, which starts a terminal
  sign-in and answers with its id.
- **One request, three credentials** (CDX-015): `token`, `api_key` and `auth_json` are fields of
  the same `CreateAccount`, each tied to its provider; more than one is refused rather than one
  chosen silently. The import endpoint refuses a malformed body without echoing it, as the token
  routes do (SPEC-502 TKP-006).
- **The Codex CLI is pinned** by version and sha512 in the source instead of followed on a channel
  (ADR-0355): the harness is checked against one release, and a bump is a code change. The
  contribution pinned 0.167.0; 0.169.0 was the newest release on 2026-11-16 and passed the same
  checks, so it is the one pinned.
- **SPEC-447 integration (CDX-013).** The domain already refused a person's choice of another
  provider (`Refusal::ProviderMismatch`) and rotation already filtered by provider; only the UI's
  list (Claude subscriptions for every task) and the words (Claude in every move notice, refusal and
  Needs you) assumed Claude. They now follow the task's provider. The Needs you action is matched
  per provider, so a Claude task's request reads exactly as before.

## Evidence

### Slice A (the adapter)

- **Port onto current `dev`** (`3177b151`): `git cherry-pick --no-commit pr-1146` gave 30
  conflicting files, resolved by keeping both sides (Context lists what met what); the whole port
  built and passed `cargo nextest` for `yulius-domain`, `yulius-contracts`, `yulius-harness`,
  `yulius-app` and `fake-agent` (1417 passed) and the `yulius` lib with the integration files
  `codex`, `codex_cli`, `accounts`, `account_token`, `agent_hooks`, `claude_gate`, `task_delete`,
  `automations`, `subscription_moves`, `subscription_pool`, `account_usage`, `usage_credits`,
  `cockpit`, `restart`, `supervision` (951 passed, 0 failed) before it was cut into slices.
- **Codex CLI 0.169.0** (npm `@openai/codex@0.169.0-linux-x64`, registry sha512
  `o05NyAOC…GFXVbVww==` matched the downloaded tarball; 45 entries, all regular files):
  `codex --version` printed `codex-cli 0.169.0`; `--help`, `login --help` and `resume --help` list
  `--no-daemon`, `-C`, `-c`, `-m`, `--dangerously-bypass-approvals-and-sandbox`,
  `--dangerously-bypass-hook-trust`, `login --device-auth` and `resume [SESSION_ID] [PROMPT]`.
  With the `config.toml` and `hooks.json` the launch plan writes and its `-c` overrides (the
  `yulius mcp` bridge, the `sh -c` wrapper with `env_vars`, a bearer HTTP server,
  `model_reasoning_effort`), `codex --no-daemon -C … mcp list` listed the three servers and exited
  0; `codex login status` read an API-key `auth.json` ("API key configured"). Source of
  `rust-v0.167.0` and `rust-v0.169.0` compared: `codex-rs/hooks/src/lib.rs` identical, the
  `AuthDotJson` struct identical, rollouts still under `sessions/YYYY/MM/DD`; `protocol.rs`
  changes touch no field the observer reads.
- **A real rollout** (`tests/fixtures/codex/exec-0.169.0.jsonl`): Codex 0.169.0 run with
  `codex --no-daemon exec --dangerously-bypass-hook-trust "Say hi"`, then `exec resume <id> "Go on"`,
  against a local stub of the Responses API (port 20107; synthetic text, usage and
  `x-codex-primary/secondary-*` headers, then a 429 `usage_limit_reached`). Its hooks were called
  with `session_id`, `transcript_path`, `cwd`, `hook_event_name` (SessionStart `source`,
  UserPromptSubmit `prompt`, Stop `stop_hook_active` and `last_assistant_message`, SessionEnd
  `reason`). `observe::a_rollout_the_real_codex_0_169_wrote_is_observed` reads it: the session and
  version 0.169.0, two typed prompts (the environment context is not one), one response's usage
  once though Codex repeats its `token_count` at the limit, both windows (42.5 % of 300 min, 12 %
  of 10,080 min), one turn end and one usage limit (`LimitScope::Account`), no malformed line.
- **Interim refusals**: `task_delete::…`, `automations::…` and `accounts::…` still answer 422 for
  `harness: codex` and `provider: openai` (the existing tests, unchanged, pass).
- `cargo fmt --all -- --check` clean; `cargo clippy --workspace --all-targets -- -D warnings`
  clean; `cargo nextest` for every crate's unit tests and the `yulius` integration files
  `accounts`, `account_token`, `account_usage`, `automations`, `task_delete`, `cockpit`,
  `supervision`, `session_docs`, `imports`, `usage_credits`: 2227 passed, 0 failed.
- **Review round 1** (Fable 5.2, `9bfa82ec5`, request changes: 4 major, 7 minor, 4 nits, all own
  code), answered in `34f784fe3`, each test failing first:
  - a resumed run counted the run before's last response (Codex repeats `token_count` at a limit):
    usage now comes from `token_usage_record` by `response_id`
    (`observe::a_resumed_codex_run_counts_no_response_of_the_run_before`, on the real fixture);
  - a blocking Stop hook's `<hook_prompt>` read as a typed prompt: 0.169's provenance decides, more
    prefixes for 0.167 (`observe::a_stop_hooks_continuation_is_not_a_typed_prompt`, on a second
    real 0.169.0 rollout `exec-0.169.0-stop-block.jsonl`);
  - a rollout's `cli_version` became the run's Claude Code version: Codex's `SessionMeta` carries
    none (`cli_version() == None` in the resumed-run test);
  - rate limits were written outside the usage lock: `App::rollout_usage` goes through
    `change_account_usage` and `rollout_reading` drops an older reading
    (`capacity::tests::a_rollout_reading_replaces_only_an_older_one`);
  - minors: cache writes counted once (`observe::codex_cache_writes_are_counted_once`); a Codex
    prompt always follows `--`, also after `resume <id>` (real 0.169.0 took `resume <id> --
    completion` as a prompt; `launch::a_codex_prompt_that_is_a_subcommand_word_stays_a_prompt`);
    a write-back needs a whole sign-in, and what the rule cannot tell apart is in ADR-0355's
    Consequences (`login::only_a_genuine_refresh_replaces_the_stored_codex_credential`); a run's
    session started since the run (`wiring::harness::tests::a_codex_session_is_the_first_rollout_of_the_worktree`);
    a sign-in's `auth.json` is read within its bound and scrubbed
    (`accounts::tests::a_codex_sign_ins_file_is_read_within_its_bound`); the `codex` PreToolUse
    guard moves to slice C1, where Codex hooks reach Yulius and can be tested end to end;
  - nits: comments, ADR "first rollout", the spec's word on SPEC-001. `codex_rollout`'s
    synchronous walk is placed with slice C1, which calls it on a run's paths.
- `scripts/dev mutants` (on `34f784fe3`) printed `308 listed, 119 caught, 0 missed`, but that
  summary is void: slice B's review found every "caught" mutant failed on one xtask test that
  cannot pass in cargo-mutants' copy (#1374, the tool's, outside this change). The round-1 tests
  each failed first against the code before its fix (above), which is what they hold.
- After the fixes: `cargo fmt --check` and `cargo clippy --workspace --all-targets -D warnings`
  clean; the same `cargo nextest` set plus `metrics_recount`: 2235 passed, 0 failed. Yulius CI
  passed on `9bfa82ec5`.
- **Review round 2** (Fable 5.2): approve of `ddc533f47`, with one minor (the fixtures README rows)
  and four nits; answered in this pull request after round 3 (below). Its merge candidate on `dev` failed the repository check
  (run `cdfcd297`): `dev` had taken ADR number 0353 meanwhile (CI admission), so this decision is
  ADR-0355; `origin/dev` merged cleanly, `python3 scripts/check_repository.py` passed, clippy clean,
  `cargo nextest` for `yulius-harness`, `yulius-domain`, `yulius-app`: 1350 passed.
- **Review round 3** (Fable 5.2, `2a40700a7`, request changes): the merge and the renumbering were
  right, but round 2's own-code findings were to be fixed here, not in slice B (ADR-0141). Fixed:
  the fixtures README lists both real rollouts and counts usage from `token_usage_record`;
  `scrub`'s doc line is back on `scrub`; a user message whose provenance is all `unknown` is
  decided by its text (`codex::tests::unknown_provenance_lets_the_text_decide`, failing first); a
  rollout line dated ahead of the server's clock counts as read now
  (`capacity::tests::a_rollout_reading_replaces_only_an_older_one` extended, failing first); the
  0355 index line is last; the pull request body names ADR-0355. Left as found: a reading of one
  window does not date the stored reading (as Claude's session readings, SPEC-473); whether a
  Codex account's rollouts carry one window is for slice C2 to see with Codex runs. Noted for slice
  C1: dev's notification text "Failed (no Claude account)" (`yulius-domain/src/notify.rs`) is to
  name a Codex task's provider once Codex tasks exist.

### Slice B (the pinned Codex CLI)

- **The pin is 0.169.0**: `the_pinned_codex_release_has_linux_packages` expects 0.169.0's tarball
  path, install name and both registry sha512s (`npm view @openai/codex@0.169.0-linux-x64
  dist.integrity` `sha512-o05NyAOC…GFXVbVww==`, `…-linux-arm64` `sha512-NZ/DaQUp…G/xFg==`) and
  failed on 0.167.0 first. Both tarballs were downloaded and matched those sha512s; each has 45
  entries, all regular files; `bin/codex` is ELF machine 62 (x86-64) and 183 (aarch64).
  Compatibility with the integration: Evidence of slice A (flags, generated files, `mcp list`,
  `login status`, hooks library and `auth.json` schema unchanged since 0.167.0, a real rollout and
  real hook payloads read).
- **Installer** (`wiring/codex_cli.rs`, the contribution's, unchanged but for 0.169.0 and
  `UpdateError::new`): `codex_cli::` 7 passed against a local registry stub (installed once as
  its own tree, concurrent callers, a refused tarball installs nothing, a failed install is not
  retried by every start, an earlier release goes once nothing runs it, open to sandbox users
  whatever the umask, an unsupported platform's reason, the provided CLI or a reason for a run);
  the read-only bind of a contained run comes with Codex tasks (slice C1); the opt-in live test
  `codex_cli::the_real_pinned_release_installs_from_npm` (`--run-ignored only`) installed the real
  0.169.0 from `registry.npmjs.org` and `bin/codex --version` printed `codex-cli 0.169.0`.
- **Wiring**: `HarnessPort::provide_codex` (default: none), the adapter's `with_codex_cli` with the
  host's `codex` as fallback, the server's `codex_cli` and `warm_codex_cli` (installs at start only
  when an `openai` account exists, so nothing is downloaded before slice C2 lets one exist);
  `codex-cli` is left out of a host move (`instance::walk`). Slice A's last two wording nits
  (a comment's citation, the fixtures README's `world_state` phrase) are fixed here.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` clean; `cargo nextest`
  for `yulius-domain`, `yulius-harness`, the `yulius` lib and `codex_cli`, `claude_updates`,
  `cockpit`, `accounts`: 1523 passed, 0 failed.
- `scripts/dev mutants` (on `5033af233`) printed `103 listed, 82 caught, 0 missed`: void, as
  the review found (all 82 failed on `xtask::cli security_is_listed_and_needs_a_vidoc_key`, which
  fails in cargo-mutants' copy without `.git`; issue #1374, outside this change). Instead, the
  lines the new tests hold were undone by hand, one at a time, on `8f52f6093`
  (`target/evidence/handmut.py`); each made its test fail (nextest exit 100):
  - the streamed download bound (`if size > self.max_tarball` → `false`):
    `a_tarball_over_the_download_bound_installs_nothing` (the `Content-Length` check alone is an
    equivalent mutant: the stream's bound refuses the same tarball);
  - keeping a running release (`running.contains(&index)` → `false`):
    `an_earlier_release_a_process_runs_is_kept`;
  - the private staging folder (its `set_mode(staging, 0o700)` removed):
    `the_staging_folder_is_private`;
  - the start rule (`== Provider::OpenAi` → `!=`):
    `the_cli_is_installed_at_start_only_for_codex_subscriptions`;
  - what a caller that must not wait gets (`ready` ignored): `codex_runs_get_the_provided_cli_or_a_reason`.
- `yulius security-scan`: not run (Vidoc refused the instance key, HTTP 401, exit 2), as for
  slice A; CI's `security` job skips (exit 78).
- **Review round 1** (Fable 5.2, `4947ed7b6`, request changes: 4 major, 3 minor, 4 nits, own code;
  the installer itself read sound), answered in `8f52f6093` and the commit after it:
  - the void mutation summary: replaced by the hand check above; the tool's bug is issue #1374;
  - the download bound, a running earlier release and the start rule had no test: three new tests
    (`a_tarball_over_the_download_bound_installs_nothing` with and without `Content-Length`,
    `an_earlier_release_a_process_runs_is_kept`, `the_cli_is_installed_at_start_only_for_codex_subscriptions`);
    the start rule moved from `serve.rs` into `App::warm_codex_cli` so a test reaches it;
  - minors: the `install=false` arm is tested (installed, recent failure, never tried, host
    `codex`), and its reason says "not installed yet" instead of "still being installed"; the
    documents say runs and sign-ins start the CLI from the later slices (the diagram's arrows are
    marked planned); CLI-002's evidence names `codex_runs_get_the_provided_cli_or_a_reason`;
  - nits: the staging folder is 0700 whatever the umask (`the_staging_folder_is_private`, run
    under umask 022, failed first at 0755); the live test's size is 167 MB; the fallback's warning
    says the CLI is not ready; slice A's last Plan item is ticked.
  - For slice C1 (direction, not defects here): the fallback `None` means the run's own `PATH`,
    while `host_codex` is looked up on the server's; C1 passes the found path to the run.
- After the fixes: `codex_cli::` 11 passed; `cargo fmt --check` and `cargo clippy --workspace
  --all-targets -D warnings` clean.

### Slice C1 (Codex tasks)

- **Brought from the contribution** by 3-way patches against today's `dev` (the full port of
  2026-11-16 was older than `dev` by then): the supervisor's Codex runs (homes per sandbox and
  account with a shared `sessions`, `auth.json`, `config.toml` and `hooks.json` written through
  handles, the CLI from `provide_codex` before the launch lock, its directory bound read-only into
  a contained run, the session id from the hook or the rollout, the write-back and the
  `CodexLeftover` guard), `harness: codex` accepted for tasks, automations and the MCP tool (a
  Claude model refused for a Codex task), `yulius hook codex` (a hook of another harness than the
  run's refused), provider-aware words (`account_gate::account_kind`, refusals, waiting notices,
  move notices, the Needs you action) and Continue / Run on… refusing another provider's
  subscription (CDX-013), the fake agent's Codex mode and the tests. Left for slice C2: Codex
  sign-ins, imports and API keys, usage readings, `codex_connected` and the capacity view's
  provider (the two tests that need them,
  `an_imported_codex_sign_in_reaches_the_run_and_its_refresh_is_kept` and
  `the_capacity_view_names_providers_and_resumes_with_codex_too`, come with it).
- **Changed for what slices A and B settled**: the fake agent writes each response's
  `token_usage_record` (unique `response_id`) as Codex 0.169.0 does, since usage is counted from
  it (`fake-agent::cli::codex_rollout_has_session_meta_turns_usage_and_a_limit` asserts it and
  failed first; `codex::a_codex_task_runs_in_its_home_…` timed out waiting for tokens before);
  rate limits go through `rollout_usage` (slice A's review) rather than `record_account_usage`.
- **Answered from earlier reviews**: the `codex` PreToolUse guard (slice A review, minor) with
  `hook_client::a_codex_bash_call_is_checked_locally_like_claude_codes` (fails without the guard,
  passes with it, no connection made); dev's notification "Failed (no Claude account)" names Codex
  for a Codex task (`notify::tests::an_ended_task_says_how_it_ended`, failed first; a Claude task
  reads as before); the staging folder is created 0700 rather than narrowed after (slice B
  review, nit). The fallback to a host `codex` stays as the contribution has it: the run resolves
  `codex` on its own `PATH`, which in a sandbox copied from the host holds the same standard
  directories the server looked in; a host `codex` elsewhere is not found (a known limit).
- **Unrelated failure met**: `migrate_helper::tests::a_stop_of_the_helper_ends_the_command_first`
  failed once under the full lib run and passed three of three alone; its files are not in this
  change: issue #1375.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings` clean; `cargo nextest`
  for every crate's unit tests and the `yulius` files `codex`, `codex_cli`, `agent_hooks`,
  `task_delete`, `automations`, `supervision`, `pool_supervision`, `subscription_moves`,
  `subscription_pool`, `claude_gate`, `accounts`, `account_token`, `mcp`, `restart`, `notify`,
  `telegram`, `cockpit`: 2403 passed, 0 failed.
- **Mutation check by hand** (`scripts/dev mutants` counts every mutant caught, #1374): on
  `254d68d6a`, five lines undone one at a time, each failing its test (nextest exit 100;
  `$HOME/.cache/yulius-task-485/handmut-c1.py`): a hook of another harness accepted →
  `agent_hooks::codex_hooks_reach_yulius_for_codex_runs_only`; another provider's subscription
  taken on Continue → `codex::a_codex_start_that_loses_its_subscription_leaves_no_credentials`;
  no write-back → `codex::only_a_genuine_codex_refresh_reaches_the_vault`; the provided CLI not
  passed to the run → `codex::a_codex_task_starts_the_provided_codex_cli`; the Codex failure words
  → `notify::tests::an_ended_task_says_how_it_ended`.
- `yulius security-scan`: not run (Vidoc refused the key, HTTP 401, exit 2).

