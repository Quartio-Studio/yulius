---
status: done
---

# SPEC-451: Codex returns as a second harness (proposal to reverse F 2026-10-08)

Status: proposed (implemented on this branch; accepting it is the founders' decision, see Decisions)
Date: 2026-11-10
Owner: studenciak00owl (external contribution)
Scope authorization: a proposal, not an accepted requirement. The founders removed Codex on
2026-10-08 (`F 2026-10-08` in SPEC-001: "prune Codex support and switching providers; we use only
Claude; if Codex gets better, we'll support it"). This pull request asks them to take it back and
makes that a merge decision: merging it accepts this spec and
[ADR-0290](../../docs/decisions/0290-codex-returns-as-a-second-harness-with-a-pinned-cli.md), which
then supersede `F 2026-10-08` for Codex (not for provider switching or session translation, which
stay removed). Until then `F 2026-10-08` stands, and SPEC-001, SPEC-002 and SPEC-003 mark each
statement this changes as "proposed (SPEC-451)". The work was built and used on the contributor's
fork (`studenciak00owl/yulius`), whose owner asked for it verbatim: "Bring back Codex (OpenAI
Codex CLI) as a second harness in this fork, working the same way Claude Code does: Codex accounts
in the subscription pool, login from the UI, a task can be created with harness Codex, the native
Codex session runs in the task terminal, observation of its transcript, usage windows, resume after
restart."

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
- **What is different now.** Codex CLI 0.167.0 (npm `@openai/codex`, source tag `rust-v0.167.0`)
  was checked against what Yulius needs (Evidence): `--no-daemon` and hooks (SessionStart,
  UserPromptSubmit, Stop, PreToolUse) with `--dangerously-bypass-hook-trust`; `codex login
  --device-auth`; `codex resume <id>`; per-response token usage and `rate_limits` in the rollout's
  `token_count`, and the usage limit as the turn's `usage_limit_exceeded` error; MCP servers and the
  shell environment policy in `config.toml`. Yulius pins that exact release (sha512 in the source),
  so no release without `--no-daemon` or with other formats reaches a run (R-12); hooks are trusted
  by the flag (R-03). R-02 is handled by a write-back rule but not verified live.
- **Re-implemented, not reverted.** More than 700 commits landed on `dev` after the removal; a
  revert conflicts in 74 files. The adapter is re-implemented on today's code with the removed one
  as reference. Since then the Claude path grew hook holds (ADR-17), the PostToolUse redaction and
  PreToolUse guard hooks, the status line feed, goal mode (SPEC-117), typed openings (SPEC-030), the
  transcript step-back (SPEC-034), sub-agent tails, recounts (SPEC-091) and session drift by hook
  (SPEC-281); those stay Claude-only unless a requirement below says otherwise.
- **What already fitted.** Rotation keeps candidates within the task's provider (`task_provider`
  in `yulius-domain/src/rotation.rs`, `Refusal::ProviderMismatch` for a person's choice), accounts
  carry a provider, and the composer groups models by provider and sends `harness` when the chosen
  provider runs another harness. SPEC-447 (moving a task between subscriptions, removing one in
  use) builds on the same rotation, so it keeps a Codex task on Codex subscriptions once its words
  and its "Run on…" list follow the task's provider (CDX-013).
- **Getting the CLI.** Sandboxes copy the host system, which normally has no Codex CLI. Claude Code
  reaches runs by a read-only bind of its npm package (SPEC-421, `wiring/claude_cli.rs`). The Codex
  release ships its native build as platform packages (`@openai/codex@0.167.0-linux-x64`, 174 MB
  packed): `package/vendor/<triple>/` holds `bin/codex` (static musl), `codex-path/rg`,
  `codex-resources/` and `codex-package.json`; the binary runs from that tree by itself.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| CDX-001 | `Harness::Codex` (`codex`) and `Provider::OpenAi` (`openai`) exist; creating a task or an automation with harness `codex`, and an account with provider `openai`, is accepted; anything else stays 422. | `yulius-domain` harness tests; `tests/automations.rs`, `tests/task_delete.rs`, `tests/accounts.rs` |
| CDX-002 | A Codex run is `<codex> [--no-daemon] -C <worktree> -c <config>… [-m <model>] [--dangerously-bypass-approvals-and-sandbox] --dangerously-bypass-hook-trust [resume <id>] [prompt]` with `CODEX_HOME=<sandbox>/codex/<account>`, the account's `auth.json` and Yulius's `config.toml` and hooks written there (mode 0600), the Yulius MCP as the `yulius mcp` stdio bridge, the sandbox's MCP servers with their values only in the secret environment, and no secret in argv. | `yulius-harness/tests/launch.rs` |
| CDX-003 | Codex homes are per (sandbox, account) and share one `sessions` directory per sandbox, so a resumed session finds its rollout whichever account runs it. A session's rollout is found by its id; a new session's id is adopted from its SessionStart hook, else from the first rollout for the worktree created after the run started. | `tests/paths.rs`; `tests/codex.rs` |
| CDX-004 | The rollout is observed: session meta, user turns (not injected context), turn completion, token usage (per response, so a resumed run counts only its own), rate-limit windows (recorded on the account) and a reached rate limit (a usage limit: the task rotates or waits). Unknown types are ignored and malformed lines counted. | `yulius-harness/tests/observe.rs` with synthetic fixtures `tests/fixtures/codex/*` |
| CDX-005 | A Codex account signs in from the Subscriptions page by `codex login --device-auth` in a server-owned terminal (the device URL and code shown, credentials kept in `auth.json` of a temporary home) or by an OpenAI API key given in the UI; the credential is a validated `auth.json` sealed in the vault. | `yulius-harness/tests/login.rs`; `tests/accounts.rs` |
| CDX-006 | After a Codex run ends, the run's `auth.json` replaces the stored one only as a genuine refresh (same ChatGPT account and `id_token` subject, newer `last_refresh` not dated ahead; never an API-key account or a signed-out one), so a refreshed token is kept and a forged file is not. A newer copy another run of the account left is kept at launch; the file is removed once no run of the account is left in the sandbox, also when a launch ends before its run is live (a failed spawn, a subscription removed just after it was chosen). | `yulius-harness/tests/login.rs`; `tests/codex.rs`; `supervisor.rs` unit tests |
| CDX-007 | Codex hooks reach Yulius as Claude Code's do (`yulius hook codex <event>`): SessionStart (briefing, session id), UserPromptSubmit, Stop (turn boundary, gate, holds) and PreToolUse; a hook of another harness than the run's is refused. | `tests/agent_hooks.rs` |
| CDX-008 | A Codex account's usage windows come from `chatgpt.com/backend-api/wham/usage` (read-only) and from the rate limits in its runs' rollouts; an API-key account has no windows. | `wiring/usage.rs` tests |
| CDX-009 | Rotation picks only accounts of the task's provider; a Codex task at its limit moves to another Codex account by native resume, and waits when none has capacity. A task never switches harness. | `yulius-domain` rotation tests; `tests/codex.rs` |
| CDX-010 | The UI offers Codex: the composer's model menu has an OpenAI group (its choice sends `harness: codex`), automations show and take a harness, the Subscriptions page adds a Codex subscription (device sign-in, API key or imported `auth.json`), and the mock serves the same. | `e2e/tests/codex.e2e.ts` |
| CDX-011 | A Codex account takes an imported `auth.json` of a ChatGPT sign-in made elsewhere (`codex login` with a separate `CODEX_HOME`): at creation (`auth_json`) or for an existing account (`POST /api/admin/accounts/{id}/import`); only a full ChatGPT sign-in, sealed, never returned or logged; its refreshes are written back (CDX-006). Detailed by IMP-001..004. | `tests/accounts.rs`; `tests/codex.rs`; `e2e/tests/codex.e2e.ts` |
| CDX-012 | Codex runs and sign-ins start the pinned Codex CLI Yulius installs from npm into `<data>/codex-cli` (sha512 pinned in the source), never a `codex` the sandbox may lack. Detailed by CLI-001, CLI-002. | `tests/codex_cli.rs`; `tests/codex.rs` |
| CDX-013 | With SPEC-447, a Codex task moves only between Codex subscriptions: "Run #N on…" and "Continue #N on…" list the task's provider's subscriptions only ("No Codex subscription yet" when none); a person's choice of another provider's subscription is refused naming it ("<name> is not a Codex subscription, so it cannot take the task."), by Continue before the task starts, so it keeps resting; removing its subscription moves it to another Codex one in the same session, never to a Claude one, and with no Codex subscription usable it waits, its terminal and its Needs you naming Codex ("Sign a Codex subscription in again or add one in Subscriptions"). What the task says names its own provider, never Claude for a Codex task; a Claude task reads as before. | `tests/codex.rs` `a_codex_task_moves_only_between_codex_subscriptions`, `a_codex_start_that_loses_its_subscription_leaves_no_credentials`; `tests/subscription_moves.rs` unchanged; `e2e/tests/codex.e2e.ts` |
| CDX-014 | `GET /api/me` `instance.claude_connected` keeps meaning a usable Claude account (onboarding, the Superagent, split proposals and the Cockpit run Claude); `instance.codex_connected` says whether a Codex account is usable, and the task view's "Connect Claude" hint is left out while one is. The capacity view (Dashboard) names each subscription's provider and, with nothing available, resumes at the first reset of either pool; a Codex usage reading the endpoint refuses (403) says so in Codex's words. | `tests/claude_gate.rs` `a_codex_account_alone_is_codex_connected_not_claude_connected`; `tests/codex.rs` `the_capacity_view_names_providers_and_resumes_with_codex_too`; `wiring/usage.rs` `a_codex_usage_403_speaks_of_codex` |
| IMP-001 | `POST /api/admin/accounts` with `{"provider":"openai","label","auth_json"}` (instance admin) creates the account `ok` at once with the file sealed as its credential; `POST /api/admin/accounts/{id}/import` `{"auth_json"}` replaces an `openai` account's credential and cancels a sign-in running for it. | `tests/accounts.rs` |
| IMP-002 | Only a ChatGPT sign-in is imported: a JSON object with `auth_mode` `chatgpt` and non-empty `tokens.id_token`, `access_token`, `refresh_token` and `account_id`; anything else, a Claude account, or both `api_key` and `auth_json` is 422. The file is never returned, logged or written to the audit (which names `auth_json` only). | `yulius-harness/tests/login.rs`; `tests/accounts.rs`; contracts `Debug` test |
| IMP-003 | An imported sign-in is what a Codex run's home gets, and the tokens Codex refreshes during the run are written back as for a device sign-in. | `tests/codex.rs` |
| IMP-004 | Add subscription → Codex offers "Import auth.json" (paste or file) with a note to sign in with a separate `CODEX_HOME`; a Codex subscription's card imports a new file. The mock serves both endpoints. | `e2e/tests/codex.e2e.ts` |
| CLI-001 | Yulius installs the pinned Codex CLI (`0.167.0`) from npm's registry into `<data>/codex-cli/<version>-<platform>`: the tarball's sha512 must equal the one pinned in the source, only regular files of `vendor/<triple>/` inside the release are unpacked, bounded, `bin/codex` must be an ELF for the platform, the move is atomic, one install at a time, earlier releases go once no process runs them. | `tests/codex_cli.rs` |
| CLI-002 | Every Codex run and Codex sign-in starts the provided `bin/codex` by its absolute path; a contained run binds its directory read-only. Without a provided CLI (an unsupported platform, or an install that failed while the host has `codex`), the host's `codex` is used; an install that failed with no host `codex` fails the start with the reason, never `codex: not found`. | `yulius-harness/tests/launch.rs`; `tests/codex.rs`; `wiring/harness.rs` tests |

## Plan

- [x] Codex harness and OpenAI provider accepted by the domain
- [x] Codex rollout observed with synthetic 0.167 fixtures
- [x] Codex launch plan checked against Codex CLI 0.167
- [x] Codex homes, session discovery and auth write-back in the supervisor
- [x] Codex hooks reach Yulius like Claude Code's
- [x] Codex accounts sign in by device code, API key or imported auth.json, with usage windows
- [x] Yulius installs the pinned Codex CLI and every Codex run and sign-in starts it
- [x] Fake agent speaks Codex and integration tests cover a Codex task
- [x] UI offers Codex in the composer, automations and Subscriptions
- [x] Codex tasks move only between Codex subscriptions (SPEC-447 integration)
- [x] Spec, ADR, SPEC-001/002/003 and documents describe the proposal
- [x] Gates green on current dev and independent review answered
- [x] Review round 1: Claude-only flags, Continue refusal, leftover credentials, provider labels
- [x] Review round 2 approved

## Acceptance criteria

- AC-451-1: `POST /api/sandboxes/{id}/tasks` with `harness: codex` creates a Codex task that runs
  the planned `codex` command in its terminal (fake agent in tests).
- AC-451-2: a Codex run's rollout drives the task's turns, usage and limit; at a rate limit the
  task resumes the same session on another Codex account, or waits when none has capacity.
- AC-451-3: a restart reattaches the run and watches its rollout; a later Continue resumes it
  natively (`codex resume <id>`).
- AC-451-4: an administrator adds a Codex subscription from the UI and sees the device code, saves
  an API key, or imports an `auth.json`; the credential is stored sealed and never shown; an
  API-key file given as an import is refused with the reason, without echoing it.
- AC-451-5: a refreshed `auth.json` left by a run is written back to the account.
- AC-451-6: on a host without Codex, a Codex task starts
  `<data>/codex-cli/0.167.0-linux-x64/bin/codex` inside its sandbox.
- AC-451-7: a person moving a Codex task sees only Codex subscriptions; moving it to a Claude one
  through the API is refused; removing its Codex subscription moves it to another Codex one, and
  removing the last one leaves it waiting for a Codex subscription, never on Claude.

## Test plan

- Unit: domain parsing; Codex observation, launch plan, login helpers, import validation and
  rollout paths in `yulius-harness`; the pinned release's platform data; usage parsing and the
  adapter's fallback to the host's `codex` in `yulius`.
- Integration (`crates/yulius/tests`, fake agent with `--transcript-kind codex`): a Codex task's
  launch, rollout watch, rotation at a limit, resume, auth write-back, hooks; accounts with
  provider `openai` (API key and import paths, refusals, audit); refusals for unknown harnesses;
  the installer against a local registry stub (install once, concurrent callers, refused tarballs,
  leftovers and earlier releases removed); a contained run reading the provided CLI through a real
  bubblewrap bind; SPEC-447 moves of a Codex task (CDX-013).
- e2e (`e2e/tests/codex.e2e.ts`): Add subscription for Codex (API key, import pasted and from a
  file, the card's import), the composer's Codex choice, an automation's agent, "Run on…" of a
  Codex task.
- No test calls OpenAI or npm, except `codex_cli::the_real_pinned_release_installs_from_npm`
  (`--ignored`, run by hand). Credentials and rollouts are synthetic.

## Decisions

- **A proposal the founders accept by merging.** The code is complete and tested, but whether
  Yulius supports Codex is a product decision (`F 2026-10-08` made the opposite one). SPEC-001,
  SPEC-002 and SPEC-003 therefore keep `F 2026-10-08` and mark each statement this spec changes as
  "proposed (SPEC-451)"; merging accepts it. Rejected: rewriting those statements as if accepted
  (it would decide for the founders); a fork-only change (the fork's owner wants it upstream).
- **Codex only, no provider switching.** A task keeps its harness for life; rotation, a person's
  move and a removal stay within the task's provider; there is no transcript translation and no
  cross-provider switch, so `F 2026-10-08` stays in force for ROT-005/006 and ADR-06.
- **Re-implemented, not reverted** (74 conflicting files); the removed code is the reference.
- **Session id.** Codex has no `--session-id`: Yulius adopts it from the SessionStart hook first,
  with the rollout `session_meta` scan as the fallback.
- **Claude-only features stay Claude-only** for Codex tasks where Codex counterparts are not
  checked live: goal launch prompts (a Codex goal task gets its request as its first prompt), typed
  openings with images, step-back, status line, sub-agent tails, session import and search, and
  #1138's detection of a refused subscription: Codex's wording for a revoked ChatGPT sign-in is
  not known without a live account, so such a run fails instead of moving on (a known limit).
- **Write-back trust.** The run's `auth.json` is trusted only as a refresh of the same sign-in
  (`tokens.account_id` and the `id_token`'s `sub`), and removed after the account's last run only
  when it equals the vault's copy. Known limit: one account used at once in two sandboxes keeps two
  copies of its rotating refresh token, and one of them can go stale (R-02; not verified live).
- **Import is a credential like the API key**: no sign-in process, no temporary home. `account_id`
  and all three tokens are required: without `account_id` the write-back rule never takes a
  refresh, and the account would go stale at the first rotated refresh token. A separate endpoint
  `POST /api/admin/accounts/{id}/import` rather than a body on `relogin`, which starts a terminal
  sign-in and answers with its id.
- **The Codex CLI is pinned** by version and sha512 in the source instead of followed on a channel
  (ADR-0290): the harness is checked against one release, and a bump is a code change.
- **SPEC-447 integration (CDX-013).** The domain already refused a person's choice of another
  provider (`Refusal::ProviderMismatch`) and rotation already filtered by provider; only the UI's
  list (Claude subscriptions for every task) and the words (Claude in every move notice, refusal and
  Needs you) assumed Claude. They now follow the task's provider. The Needs you action is matched
  per provider, so a Claude task's request reads exactly as before.

## Evidence

Run on 2026-11-10 on `feat/codex-harness-upstream`, from `upstream/dev` `91a0cec5` (SPEC-447
included). The fork's branch `feat/codex-harness` (`da0e55a4..b4396191`, without the separate
Safari fix `4fdd9381`, upstream PR #1142) was applied with a three-way merge; two files
conflicted (`pages/admin.js`, `wiring/harness.rs`) and were resolved by keeping both sides.

- Codex CLI 0.167.0, checked on the fork (2026-11-09): `codex --help`, `codex exec --help`,
  `codex login --help`, `codex resume --help` read; the rollout, hook, auth, MCP and shell-policy
  formats checked in the `rust-v0.167.0` source; the real CLI accepted Yulius's `-c` overrides
  (the `yulius mcp` stdio bridge, the `sh -c` wrapper with `env_vars`, a bearer HTTP server,
  `model_reasoning_effort`), its `config.toml` and `hooks.json` (`codex --no-daemon … mcp list`
  listed them without a parse error) and an API-key `auth.json` (`codex login status`: "API key
  configured"). Live npm (`codex_cli::the_real_pinned_release_installs_from_npm`, `--ignored`):
  the real `@openai/codex@0.167.0-linux-x64` matched the pinned sha512, unpacked, and `bin/codex
  --version` printed `codex-cli 0.167.0`. Not rerun here (no change to the installer).
- CDX-013 test-first: `codex::a_codex_task_moves_only_between_codex_subscriptions` failed on the
  merged branch before the fix ("Claude test is not a Claude subscription, so it cannot take the
  task." for a Codex task) and passes after it; the e2e "a Codex task runs only on Codex
  subscriptions" failed before the fix ("Run #12 on…" listed the Claude subscriptions) and passes.
- Review round 1 (fresh read-only session, REQUEST_CHANGES: 1 major, 5 minor, nits), answered
  test-first: `claude_gate::a_codex_account_alone_is_codex_connected_not_claude_connected` failed
  (`claude_connected` was true for a Codex account alone, which hid "Connect Claude" while
  onboarding, the Superagent and the Cockpit still need Claude) and passes with `claude_connected`
  Claude-only again and `codex_connected` added (CDX-014);
  `codex::a_codex_start_that_loses_its_subscription_leaves_no_credentials` failed first on
  Continue with a Claude subscription (the task became `failed_precondition`), then on the removed
  subscription's `auth.json` left in the sandbox by a launch that ended before its run was live,
  and passes; `codex::the_capacity_view_names_providers_and_resumes_with_codex_too` and
  `wiring::usage::a_codex_usage_403_speaks_of_codex` are new; the stranded-notice check now covers
  every notice. A failed spawn has no test of its own: it goes through the same guard
  (`CodexLeftover`, disarmed only once the run is live), and the real ptyhost gives no
  deterministic way to fail a spawn after the files are written. Docs: `/continue`, `/switch`, `/api/me`, the status contract; the automation MCP
  tool takes `codex`; a refused Codex sign-in is recorded as a known limit (Decisions).
- `cargo fmt --all -- --check`: clean. `cargo clippy --workspace --all-targets --locked -- -D
  warnings`: clean. `cargo xtask macos` (clippy for `aarch64-apple-darwin`, `-D warnings`): clean
  (a Linux-only test's import, `build_tuned`, moved into the test). `cargo xtask deps`:
  advisories, bans, licenses, sources ok; `cargo shear` no issues.
  `python3 scripts/check_repository.py`: passed.
- Unit: `cargo test -p yulius-domain -p yulius-contracts -p yulius-harness -p yulius-app -p
  fake-agent`: 1167 passed, 0 failed; `cargo test -p yulius --lib -p yulius-server`: 258 passed.
- Integration (`cargo test -p yulius --test main --` with the filters): `codex:: codex_cli::
  accounts:: agent_hooks:: claude_gate:: task_delete:: automations:: subscription_moves::
  subscription_pool account_usage:: analytics::`: 127 passed, 0 failed, 1 ignored (the live npm
  install); `supervision:: run_metrics:: restart:: claude_updates:: metrics_recount:: cockpit::
  instance remote_mcp::` (`supervision::` includes `pool_supervision::`): 170 passed, 1 ignored;
  `mcp`: 63 passed, 2 ignored. SPEC-447's own `subscription_moves::` tests pass unchanged, so a
  Claude task reads as before.
- e2e (`npx e2e run`, Node 22.23, Chromium), after the review's fixes: `codex`, `subscriptions`,
  `claude-connect`, `onboarding-flow`, `mcp`, `quality`: 72 passed; `new-task`, `places`,
  `responsive`, `automations`: 64 passed. Before them: `subscription-names`, `consistency`,
  `archive` passed too; `new-task` once timed out 2 of 3 while five files ran beside it, then
  passed alone and in the run above.
- Review round 2 (same fresh session, read-only): APPROVE for `2c5d7a18`, with two nits: a doc
  comment the guard displaced (fixed) and no failed-spawn test (answered above).
- Numbers: SPEC-451 and ADR-0290 (the highest on `upstream/dev` and in its open pull requests,
  #1143–#1145 holding SPEC-449, SPEC-450 and two ADR-0289s, plus one); no migration. On the final
  tree after the renumbering: clippy (workspace, `-D warnings`) clean and `codex::
  claude_gate:: subscription_moves::` 31 passed.
- `scripts/dev security`: **not run** (exit 2: no `VIDOC_API_KEY` in the contributor's environment);
  unrun, not passed. Upstream CI's `security` job scans the pull request.
- Not verified (needs a live Codex account): a real `codex login --device-auth`; a real rollout
  and hook payloads of the TUI; token refresh and write-back; `wham/usage`; the usage-limit error
  and a "Resume paused goal?" popup after a resume; a laptop `auth.json` imported and used by a
  run; the provided CLI inside a namespace-isolated sandbox of a real instance; the refresh-token
  race of one account in two sandboxes (R-02).
