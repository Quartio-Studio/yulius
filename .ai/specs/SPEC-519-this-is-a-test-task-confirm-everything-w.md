---
yulius-task: 198f6d63-a78b-462e-ae91-4905a69d1240
status: in-progress
---

# SPEC-519: Gemini 4 Neon is offered and becomes Antigravity's default model (on agy 1.5.12)

Status: in-progress
Date: 2026-11-19
Owner: Yulius task #501
Scope authorization: the person's request of 2026-11-19 (task #501), verbatim: "this is a test
task, confirm everything work on gemini. if so, add Gemini 4 Neon to the models and make Neon the
default model".
ADR: [0373](../../docs/decisions/0373-gemini-4-neon-is-antigravitys-default-model-on-agy-1-5-12.md).

## Goal

Confirm that an Antigravity (Google Gemini) task works end to end in a live Yulius session, offer
Gemini 4 Neon (`gemini-4-neon`) in the composer and Settings under Google alongside Gemini 4
Carbon (`gemini-4-carbon`), and make Gemini 4 Neon at high effort the default model of an
Antigravity task on both Google sign-in and Gemini API key subscriptions. Claude Code (`Opus 5.5`)
stays Yulius's unnamed-agent default (`F 2026-11-19 Harness`, SPEC-514).

## Context

- **Live verification on Gemini (task #501).** Task #501 itself runs on `Harness::Antigravity` on a
  live Google sign-in subscription inside a contained sandbox:
  - The sealed Google sign-in token file was restored into the run's private `.gemini` home as
    `.gemini/antigravity-cli/antigravity-oauth-token` (`0600`), bound over `<sandbox>/home/.gemini`
    while keeping `HOME` equal to `<sandbox>/home` for Git and shell tools (AGY-003, AGY-008).
  - `brain` and `conversations` in `.gemini/antigravity-cli` resolve as symlinks to
    `<sandbox>/antigravity/shared/{brain,conversations}` (AGY-003).
  - `SessionStart` injected the Yulius briefing as a `<SYSTEM_MESSAGE>`, recorded the run's
    `conversationId` on the live attempt, `PreToolUse` (`^run_command$`) allowed shell commands
    through `yulius hook antigravity PreToolUse`, and the `yulius mcp` stdio bridge served
    `steps_get`, `step_update`, `tasks_list`, `task_get`, `sessions_search` and `record_number`
    with the task's token (AGY-002, AGY-005, AGY-006).
- **Current Antigravity model list and default (SPEC-505 AGY-002, AGY-010).**
  `DEFAULT_ANTIGRAVITY_MODEL` in `yulius-domain::harness` is `"gemini-4-carbon"` and
  `DEFAULT_ANTIGRAVITY_EFFORT` is `"high"`. The web UI's `models.js` lists only
  `gemini-4-carbon` ("Gemini 4 Carbon") under the `google` provider (`harness: 'antigravity'`), and
  the browser mock (`mock.js`) defaults an Antigravity task without `model` to `gemini-4-carbon`.
- **Gemini 4 Neon in the Antigravity CLI (`1.5.10` vs `1.5.12`).**
  - On the live Google sign-in subscription in this session, `antigravity models` and native swarm
    model enumeration list `gemini-4-neon-{low,medium,high,xhigh}` ("Gemini 4 Neon (Low|Medium|High|XHigh)").
  - However, on the currently pinned `agy` **1.5.10** (`1.5.10-5253791805734912`), launching with a
    `GEMINI_API_KEY` subscription (`modelProvider: "gemini"`) and `--model gemini-4-neon --effort high`
    exits immediately at startup with code 1:
    `error: invalid model selection (--model "gemini-4-neon" --effort "high"): --effort is not supported for model "gemini-4-neon"`.
  - Release **1.5.12** (`1.5.12-5281398513336320`, published 2026-11-18 on Google's updater
    manifests `manifests/linux_amd64.json` and `manifests/linux_arm64.json`) specifically adds
    `Gemini 4 Neon (Low, Medium, High, and XHigh)` to `/model` and `--model` for `GEMINI_API_KEY`
    connections. Probed against a local Gemini API stub in a temporary directory: both platform
    tarballs (`linux-x64` sha512 `26de25fc…db73428`, `linux-arm64` sha512 `1030d89c…d5819d9`) match
    Google's manifests and unpack a single `antigravity` binary; `1.5.12 --model gemini-4-neon
    --effort high` with `GEMINI_API_KEY` sends `/v1beta/models/gemini-4-neon:streamGenerateContent`,
    fires `SessionStart`, `PreInvocation` and `Stop` with `"modelName": "gemini-4-neon-high"`,
    writes `PLANNER_RESPONSE` steps with `"model": "gemini-4-neon-high"`, and honors
    `AGY_CLI_DISABLE_AUTO_UPDATE=true` and the same `oauth-token` file layout.
- **Explicit non-goals.** Tasks created without an agent and without a model still run Claude Code
  on `claude-opus-5-5` (`F 2026-11-19 Harness`, SPEC-514 PIN-001, ADR-0368); Codex defaults stay as
  set in SPEC-511 (`gpt-6.2-astra` at `high`).

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| NEO-001 | `DEFAULT_ANTIGRAVITY_MODEL` is `"gemini-4-neon"` and `DEFAULT_ANTIGRAVITY_EFFORT` stays `"high"`. An Antigravity launch (new or resumed) without `model` passes `--model gemini-4-neon` and without `effort` passes `--effort high`; a task's own `gemini-*` model (such as `gemini-4-carbon`) or effort is passed instead; non-`gemini-*` models on an Antigravity task stay 422 (`an Antigravity task takes a Gemini model (such as gemini-4-neon)`). | `yulius-domain` unit tests; `yulius-harness/tests/launch.rs`; `crates/yulius/tests/task_delete.rs` |
| NEO-002 | An Antigravity task without `model` records `gemini-4-neon` on its attempt, shows `model: "gemini-4-neon"` with `model_default: true` and `effort: "high"` in its detail view, and is weighed by `DEFAULT_ANTIGRAVITY_MODEL` in the subscription pool. | `crates/yulius/tests/antigravity.rs` |
| NEO-003 | `yulius_domain::agent_cli::antigravity` pins Antigravity CLI `1.5.12` (`1.5.12-5281398513336320`) with the `linux-x64` and `linux-arm64` tarball `sha512` digests from Google's release manifests, installed into `<data>/antigravity-cli/1.5.12-<platform>`; existing `1.5.10` runs finish undisturbed while `pinned_cli` prunes unreferenced older directories on install. | `yulius-domain` unit test; `crates/yulius/tests/antigravity_cli.rs` |
| NEO-004 | The web UI's `models.js` lists `gemini-4-neon` ("Gemini 4 Neon") first under Google (`harness: 'antigravity'`) and keeps `gemini-4-carbon` ("Gemini 4 Carbon") second so both can be chosen and existing browser preferences (`yulius.composer.model`) stay valid; the composer's unnamed `Default` item stays Claude Code (`Opus 5.5`); the browser mock (`mock.js`) defaults an Antigravity task without `model` to `gemini-4-neon` at `high`. | `crates/yulius/tests/ui_contract.rs`; `e2e/tests/antigravity.e2e.ts` |

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] Pin Antigravity CLI 1.5.12 and default Antigravity tasks to Gemini 4 Neon
- [x] Offer Gemini 4 Neon first under Google in the UI and mock, keeping Gemini 4 Carbon
- [x] Record ADR-0373 and update product, architecture and API documentation
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- **AC-1** (NEO-001, NEO-002) Creating an Antigravity task without `model` launches `agy --model
  gemini-4-neon --effort high …`, records `gemini-4-neon` on the attempt, and returns
  `model: "gemini-4-neon"`, `model_default: true`, `effort: "high"` on `GET /api/tasks/{id}`;
  creating one with `model: "gemini-4-carbon"` or `model: "gemini-4-neon"` without `harness`
  implies `Harness::Antigravity` and launches that model; `harness: "antigravity"` with
  `model: "claude-opus-5-5"` is refused with 422.
- **AC-2** (NEO-003) `PinnedCli::antigravity` installs `<root>/1.5.12-linux-x64/antigravity` once
  when the tarball matches `1.5.12`'s pinned `sha512`, refuses any mismatched digest, and `agy
  1.5.12` accepts `--model gemini-4-neon --effort high` with a `GEMINI_API_KEY` against a local
  stub.
- **AC-3** (NEO-004) In the composer's model menu, Google lists `Gemini 4 Neon` first and `Gemini 4
  Carbon` second; picking `Gemini 4 Neon` starts an Antigravity task on `gemini-4-neon` at `high`
  effort, picking `Gemini 4 Carbon` starts one on `gemini-4-carbon` at `high` effort, and leaving
  the menu on `Default` still starts a Claude Code task (`SPEC-514`).
- **AC-4** (Live Gemini confirmation) All stages of this task (spec check, Rust and e2e tests,
  fresh-session independent review subagent, security scan, pull request, Yulius CI and merge) run
  from this Antigravity session, with any limitation recorded in `Evidence`.

## Test plan

- Unit (`yulius-domain`, `yulius-harness`): `DEFAULT_ANTIGRAVITY_MODEL` and `Harness::implied_by` /
  `Harness::unnamed` for `gemini-4-neon` and `gemini-4-carbon`; `antigravity::platform` paths and
  `sha512` digests for `1.5.12`; `plan_launch` argv for default (`gemini-4-neon` at `high`) and
  explicit Gemini models.
- Integration (`crates/yulius/tests`): `antigravity.rs` (attempt and task detail default to
  `gemini-4-neon`), `antigravity_cli.rs` (`1.5.12` stub install, digest refusal, and `--ignored`
  live download from Google storage), `harness_pin.rs` (`gemini-4-neon` implies Antigravity while
  unnamed tasks stay Claude Code), `ui_contract.rs` (`models.js` lists `DEFAULT_ANTIGRAVITY_MODEL`
  first under Google, keeps `gemini-4-carbon`, and `mock.js` matches `DEFAULT_ANTIGRAVITY_MODEL`).
- End-to-end (`e2e/tests/antigravity.e2e.ts`): choosing `Gemini 4 Neon` in the composer starts an
  Antigravity task on `gemini-4-neon` at `high`; `Gemini 4 Carbon` remains selectable and starts one
  on `gemini-4-carbon` at `high`; `Default` still starts Claude Code.
- Real CLI against a local stub: `agy 1.5.12` verified in a temporary directory with
  `GEMINI_API_KEY` and `--model gemini-4-neon --effort high` (hooks and transcript step fields).

## Decisions

- **"Make Neon the default model" sets Antigravity's default (`DEFAULT_ANTIGRAVITY_MODEL`), not
  Yulius's unnamed-agent default.** Three hours earlier on the same day (`F 2026-11-19 Harness`,
  task #496, SPEC-514, ADR-0368), the same person decided that tasks created without an explicit
  agent or model must always run Claude Code (`claude-opus-5-5`) rather than switching to Gemini,
  and in task #493 (SPEC-511, ADR-0366) set Codex's default model the same way. In this Gemini test
  task ("confirm everything work on gemini. if so, add Gemini 4 Neon to the models and make Neon
  the default model"), "the default model" therefore amends Antigravity's `gemini-4-carbon` default
  from `F 2026-11-17 Antigravity` (SPEC-505, ADR-0361). Alternative: changing `Harness::unnamed`
  and the composer's `Default` to Antigravity on Gemini 4 Neon; rejected because it would overturn
  SPEC-514 and break every instance that has only Claude subscriptions connected. Recorded in
  [ADR-0373](../../docs/decisions/0373-gemini-4-neon-is-antigravitys-default-model-on-agy-1-5-12.md).
- **Keep `Gemini 4 Carbon` in `models.js` below `Gemini 4 Neon`.** The request says "add Gemini 4
  Neon to the models and make Neon the default model", not "replace Carbon": keeping
  `gemini-4-carbon` second under Google lets people still pick Carbon (just as Anthropic lists
  previous Fable/Sonnet models below Opus 5.5) and preserves browsers that saved
  `yulius.composer.model = "gemini-4-carbon"`.
- **Bump the pinned Antigravity CLI from `1.5.10` to `1.5.12` in the same change.** Keeping
  `1.5.10` while changing `DEFAULT_ANTIGRAVITY_MODEL` to `gemini-4-neon` would make every default
  Antigravity task on a `GEMINI_API_KEY` subscription fail at launch with exit code 1 (`--effort is
  not supported for model "gemini-4-neon"`). Release `1.5.12` adds `gemini-4-neon` (`low`,
  `medium`, `high`, `xhigh`) for `GEMINI_API_KEY` connections and keeps the same binary layout,
  hooks, transcript format, auto-update variable and token file path.

## Evidence

- **Live Antigravity verification (task #501):** running on `Harness::Antigravity` on a live Google
  sign-in subscription inside bwrap containment; `.gemini/antigravity-cli/antigravity-oauth-token`
  (`0600`) restored, `brain` and `conversations` linked to `<sandbox>/antigravity/shared`,
  `SessionStart` injected the Yulius briefing, `PreToolUse` (`^run_command$`) allowed commands, and
  `yulius mcp` served `steps_get`, `steps_set`, `step_update`, `spec_check`, `tasks_list`,
  `task_get`, `sessions_search` and `record_number`.
- **Real CLI 1.5.10 vs 1.5.12 against a local stub:** `agy` 1.5.10 with `GEMINI_API_KEY` and
  `--model gemini-4-neon --effort high` exited 1 (`--effort is not supported for model
  "gemini-4-neon"`); `agy` 1.5.12 (`1.5.12-5281398513336320`, both `linux-x64` and `linux-arm64`
  tarball `sha512` digests verified against Google's manifests) ran `--model gemini-4-neon --effort
  high` with `GEMINI_API_KEY`, sent `/v1beta/models/gemini-4-neon:streamGenerateContent`, fired
  `SessionStart`/`PreInvocation`/`Stop` with `"modelName": "gemini-4-neon-high"`, and wrote
  `PLANNER_RESPONSE` steps with `"model": "gemini-4-neon-high"`.
- **Test-first red runs:**
  - `cargo test -p yulius-domain -- antigravity` failed `the_antigravity_default_model_is_gemini_4_neon_at_high_effort`
    and `the_pinned_antigravity_cli_names_its_tarballs` before the domain update.
  - `cargo test -p yulius-harness --test launch -- antigravity` failed `antigravity_new_session_on_an_api_key`
    and `antigravity_resume_on_a_google_sign_in` (`left: gemini-4-carbon, right: gemini-4-neon`).
  - `cargo test -p yulius --test main` failed `antigravity_cli::*` (3 tests),
    `antigravity::an_antigravity_goal_task_gets_its_request_as_its_first_message` and
    `task_delete::tasks_run_claude_code_or_codex` before the app/domain changes.
  - `ui_contract::the_composer_offers_antigravity_on_its_default_model` and
    `e2e/tests/antigravity.e2e.ts` (`expected ["Gemini 4 Carbon"] to equal ["Gemini 4 Neon","Gemini 4 Carbon"]`)
    failed before the `models.js` and `mock.js` changes.
- **Green runs:**
  - `cargo nextest run -p yulius-domain -p yulius-harness`: passed.
  - `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^antigravity::/) | test(/^antigravity_cli::/) | test(/^harness_pin::/) | test(/^task_delete::tasks_run_claude_code_or_codex/) | test(/^ui_contract::/))'`: passed.
  - `cargo nextest run -p yulius --run-ignored only -E 'binary_id(yulius::main) & test(/^antigravity_cli::the_real_pinned_release_installs_from_google$/)'`: passed (`agy` 1.5.12 downloaded from Google storage, `sha512` verified, `--version` returned `1.5.12`).
  - `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/antigravity.e2e.ts tests/codex.e2e.ts tests/model-fallback.e2e.ts`: 12/12, 13/13 and 4/4 passed.
  - `scripts/dev quick` and `cargo clippy -p yulius-domain -p yulius-harness -p yulius-app -p yulius-server -p yulius --all-targets -- -D warnings`: passed.
- **Mutation testing** (`scripts/dev mutants`, on `dac2b1b61`): `mutants: 15 listed, 6 caught, 0 missed, 0 timed out, 9 unviable, 0 not reached (bound 30 min, took 876 s)`.
- **Security scan** (`scripts/dev security`): exit 2 (`Vidoc refused the key (HTTP 401)`) — unrun,
  not passed.
- **Review round 1** (`yulius-reviewer` fresh session, head `dac2b1b61`, request changes; 1
  own-code major, 0 pre-existing): `SPEC-519` lines 27 and 147 named the live subscription's
  personal `@gmail.com` address (SPEC-244 SUB-005/006, ADR-0185). Redacted on both lines in a
  follow-up commit (`git diff origin/dev | grep -i '@gmail'` empty; prose-only fix, no runtime test
  applies).
- **Not tested:** a live `GEMINI_API_KEY` call to Google's production endpoint (stubbed locally
  against `agy` 1.5.12), and `linux-arm64` binary execution (verified by pinned manifest `sha512`
  and tarball inspection).

