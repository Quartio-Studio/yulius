---
yulius-task: 5f9512b5-168c-4d48-996b-cc0bc5b0de62
status: done
---

# SPEC-520: Random harness selection across all harnesses with available subscriptions

Issue: [#1437](https://github.com/Quartio-Studio/yulius/issues/1437)
ADR: [0377](../../docs/decisions/0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md)

[SPEC-583](SPEC-583-cli-nad-superagents-should-spawn-tasks-i.md) supersedes the RHM-004 default draw for direct CLI and Superagent task creation.

[SPEC-587](SPEC-587-codex-should-be-default-for-example-bart.md) supersedes the initial Random selection in the browser under RHM-006.

## Goal

Restore the random harness feature as the default for new tasks and automations, drawing uniformly across **all** added harnesses (`claude`, `codex`, `antigravity`) that currently have at least one non-exhausted subscription. Every eligible harness gets the exact same selection probability (`1 / k`), even when one harness has many more subscriptions than another, while any harness with zero subscriptions or whose subscriptions are all finished (at limit or exhausted) is excluded whenever another harness has capacity.

## Context

- **Why Random disappeared.** SPEC-505 slice E (PR #1407, ADR-0361) introduced random harness selection as the default (`auto` / `Random`). Later, task #496 ("make sure harnesses doesnt change themselves between tasks (like claude -> gemini) because it's unpredictable. A harness is set for a task") intended for a task's harness to stay fixed for that task (and its agent-spawned follow-ups), but SPEC-514 (PR #1424, ADR-0368) removed the random draw completely from `lifecycle.rs`, `automations.rs`, `models.js`, `taskview.js`, `settings.js`, and `automationedit.js`.
- **Two defects in the earlier SPEC-505 E draw too.**
  1. `Harness::DRAWN` in SPEC-505 E listed only `[ClaudeCode, Antigravity]` and left `Codex` out, and drew via `usize::from(roll) % usable.len()` over a single `u8` (which has modulo bias when `len == 3`: `256 % 3 == 1`).
  2. `App::draw_harness` checked `AgentAccounts::Usable` (`account.status.healthy()`), which treats `AccountStatus::Limited` and subscriptions at or above `rotation::THRESHOLD` (`98%`) as usable (`crates/yulius-app/src/account_gate.rs:27`). A harness whose subscriptions were all finished was still drawn and immediately paused in `waiting_capacity` even while other harnesses had spare capacity.
- **In scope:** unbiased uniform harness draw across `Harness::ALL` filtered by non-exhausted subscriptions, task draw recording (`task_harness_draws` / `harness_choice`), automation `auto` (`Random`) support, preserving a calling task agent's own harness when `task_create` / `automation_create` omits `harness` and `model`, composer / Settings / automation editor UI, and `mock.js` parity.
- **Out of scope:** switching an existing task's harness after creation (provider switching remains forbidden by SPEC-002 ADR-06); OpenAI model inference when `model: "gpt-*"` is passed without `harness` (tracked separately in [#1427](https://github.com/Quartio-Studio/yulius/issues/1427)).

## Requirements

- **RHM-001** When a task is created via `POST /api/sandboxes/{id}/tasks` without an explicit harness (`""` or `"auto"`) and without a `model`, Yulius draws its harness uniformly at random (`1 / k`, unbiased via rejection sampling over `CryptoPort::random_bytes()`) among the `k` harnesses in `Harness::ALL` (`ClaudeCode`, `Codex`, `Antigravity`) that have at least one healthy signed-in subscription with available capacity for that harness's default model (`credential.is_some()`, `status.healthy()`, not `AccountStatus::Limited` without usage data, and `!usage.exhausted_for(now, rotation::THRESHOLD, default_family)` or its fallback family). Each eligible harness has equal weight `1 / k` regardless of how many subscriptions belong to its provider.
- **RHM-002** A harness with zero subscriptions, only unsigned/`relogin`/`error` subscriptions, or whose subscriptions are all finished (`Limited` or exhausted at `rotation::THRESHOLD`) is excluded from the RHM-001 draw whenever at least one other harness has available capacity. If every connected harness's subscriptions are currently finished (`k == 0`) while `m >= 1` harnesses have healthy (`AgentAccounts::Usable`) subscriptions waiting to reset, Yulius draws uniformly (`1 / m`) among those `m` harnesses so the created task queues in `waiting_capacity` until reset. When no harness has any healthy signed-in subscription (`m == 0`), Yulius falls back to `Harness::ClaudeCode`, which `ensure_agent_account` refuses with 409 `AccountRequired`.
- **RHM-003** A task whose harness was drawn at creation keeps that harness for life (across runs, restarts, continues, limit rotations, and manual moves within its provider), records its draw in `task_harness_draws`, and reports `harness_choice: "random"` in `TaskSummary` and in the JSON/CSV analytics exports. A task given an explicit harness (`claude`, `codex`, `antigravity`) or a `model` implying one (`Harness::implied_by`) is not drawn and reports `harness_choice: "chosen"`.
- **RHM-004** When a running task's agent calls `task_create` or `automation_create` and omits `harness` (and, for `task_create`, omits `model`), Yulius uses the calling task's own harness (`harness_choice: "chosen"`) so agent-created follow-up tasks do not switch harnesses by surprise. When `harness: "auto"` is explicitly passed, or when there is no calling task (Superagent, Global Superagent, Remote MCP) and neither `harness` nor `model` is given, `task_create` draws via RHM-001 (`random`) and `automation_create` stores `auto`.
- **RHM-005** Automations accept `auto` (`AutomationRecord.harness: Option<Harness>`), the default for `POST /api/sandboxes/{id}/automations` (empty or `"auto"`) and `POST /api/sandboxes/{id}/automation-templates/{id}/enable`; each run of an `auto` automation draws a harness for its created task via RHM-001 and records `harness_choice: "random"`. `PUT /api/automations/{id}` with an empty/omitted `harness` preserves the automation's stored `harness` (`auto` or named). Existing automations stored as `claude`, `codex`, or `antigravity` keep their stored harness until changed.
- **RHM-006** The composer and Settings offer `Random` (`Random agent` chip) when no model is chosen (`selected === null`), naming all three harnesses and their default models (`Claude Code (Opus 5.5), Codex (GPT-6.2 Astra) or Antigravity (Gemini 4 Carbon), drawn for each task`); with no Claude account connected and Codex or Antigravity connected, a split-worthy prompt on `Random` skips the Claude-only split proposal and creates one drawn task. The automation editor's Agent radio group offers `Random` (`auto`, selected by default for new automations), `Claude Code`, `Codex`, and `Antigravity`. `web/assets/mock.js` mirrors RHM-001..005.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write and validate SPEC-520 and commit the failing regression tests
- [x] Draw uniformly across all harnesses with non-exhausted subscriptions in task and automation creation
- [x] Restore Random in the composer, Settings, automation editor, and browser mock
- [x] Record ADR-0377 and update product, architecture, API, and spec documentation
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Reconcile merge with SPEC-521 (#1439), verify UI and Rust suites, and review resolved head

## Acceptance criteria

- **AC-1** (RHM-001, RHM-003) With 5 healthy non-exhausted Claude subscriptions, 1 Codex subscription, and 1 Antigravity subscription, `Harness::unbiased_index` gives every one of the 3 harnesses exact `1/3` probability (`85 / 255` over accepted single-byte rolls), and queued rolls `0, 1, 2` create `claude`, `codex`, and `antigravity` tasks respectively, each with `harness_choice: "random"` in `TaskSummary` and analytics JSON/CSV; explicit harnesses and model-implied harnesses report `"chosen"`.
- **AC-2** (RHM-002) When all subscriptions of a harness are finished (`AccountStatus::Limited` or binding window `>= 98%`, including `weekly_opus >= 98%` for Claude's default Opus 5.5) or when a harness has 0 subscriptions (or only `relogin`/`error`/`pending_login`), that harness is never drawn while another harness has non-exhausted capacity; when one subscription of a harness is finished and a second subscription of the same harness has capacity, the harness remains in the uniform draw. When only Antigravity is connected and its sole subscription is `Limited` (waiting to reset), a task on `Random` draws `antigravity` (`queued` / `waiting_capacity`) instead of failing with 409 `Connect a Claude account first`; with 0 usable accounts anywhere, creation fails with 409 `AccountRequired`.
- **AC-3** (RHM-004) An Antigravity task's `task_create` and `automation_create` with no `harness` and no `model` keep `antigravity` (`chosen`); with explicit `harness: "auto"`, `task_create` draws (`random`) and `automation_create` stores `"auto"`; the Superagent's `task_create` without `harness` or `model` draws (`random`).
- **AC-4** (RHM-005) Creating an automation without `harness` (or with `"auto"`), or enabling a ready-made template, stores `"auto"` and draws a harness per run (`random`); `PUT /api/automations/{id}` without `harness` keeps the stored harness (`"antigravity"` stays `"antigravity"`, `"auto"` stays `"auto"`), while `PUT` with `"auto"` or `"codex"` updates it.
- **AC-5** (RHM-006) In the browser, the composer and Settings show `Random agent` before a model is picked, creating drawn tasks across Claude Code, Codex, and Antigravity; with Claude disconnected and Codex or Antigravity connected, a multi-part request on `Random` creates one drawn task without a Claude-account error; the automation editor defaults to `Random`.

## Test plan

- **Domain unit tests** (`crates/yulius-domain/src/harness.rs`): `unbiased_index` exact `85 / 85 / 85` uniformity for 3 harnesses (`255` rejected), `128 / 128` for 2 harnesses, `256` for 1 harness, `None` on empty candidates or all-`255` bytes; `Harness::drawn` fallback to `ClaudeCode` on empty slice.
- **Integration tests** (`crates/yulius/tests/harness_draw.rs`, replacing `harness_pin.rs`; `crates/yulius/tests/ui_contract.rs`): AC-1..AC-4 over HTTP and MCP tools (uneven subscription counts 5:1:1, exhausted `Limited` and `98%` `weekly_opus` accounts excluded, partial-pool harness kept, all-limited connected harness queued instead of 409, 0-account 409, task agent vs Superagent `task_create` / `automation_create`, automation `auto` runs and `PUT` preservation, analytics JSON/CSV, delete cleanup); `ui_contract.rs` verifying `models.js` and `taskview.js` against domain default model constants.
- **End-to-end browser tests** (`e2e/tests/antigravity.e2e.ts`, `e2e/tests/codex.e2e.ts`): AC-5 (composer `Random agent` chip and menu, 3-harness draw, exhausted-provider skip, split-card bypass without Claude when another harness is connected, automation editor `Random` default).

## Decisions

- **Draw across all 3 harnesses, one equal vote per eligible harness (ADR-0377, superseding ADR-0368 and ADR-0361's 2-harness draw).** The user explicitly requested randomisation across all added harnesses with equal probability per harness regardless of subscription count, excluding harnesses with 0 subscriptions or finished subscriptions. Rejection sampling over `CryptoPort::random_bytes()` (`byte < 256 - (256 % k)`) avoids the `256 % 3 == 1` modulo bias of a raw byte.
- **Eligibility uses the rotation policy's own non-exhausted rule, with a `Usable` fallback when every connected pool is temporarily full.** Checking only `AgentAccounts::Usable` (`status.healthy()`) included `Limited` and `>= 98%` accounts, causing drawn tasks to stall on an exhausted harness while another harness was idle. Conversely, if *every* connected harness is temporarily `Limited`, falling back to `AgentAccounts::Usable` queues the task on a connected harness rather than falsely claiming no Claude account is connected.
- **Preserve caller-harness inheritance only when a running task's agent omits `harness` and `model`.** Keeping `caller.task`'s harness on an unadorned follow-up `task_create` / `automation_create` prevents multi-slice agent work from unexpectedly switching `claude -> gemini`, while explicit `harness: "auto"`, UI creations, automations on `Random`, and Superagent calls all draw at random.
- **Do not rewrite migration 0068.** Schema version 68 has already shipped on `dev`; rewriting existing automations back to `auto` in a new migration would override automations people created intentionally as `claude` after SPEC-514. New automations and templates default to `auto` again, and existing ones can be switched to `Random` in the editor.


## Evidence

- **Red-first reproduction** (commit `e23034d6c` before production fix):
  - `cargo test -p yulius --test main harness_draw::` failed `0 passed; 6 failed` (`harness_choice` `"chosen"` instead of `"random"`, automation `"claude"` instead of `"auto"`, `409 claude_account_required` when only Antigravity was connected).
- **Domain & integration verification**:
  - `cargo nextest run -p yulius-domain -p yulius-store -p yulius-app` → passed (including `unbiased_draw_gives_every_eligible_harness_the_same_chance`, `85 / 85 / 85` exact uniformity over `0..=254` with `255` rejected, and `validation_rejects_bad_input`).
  - `cargo test -p yulius --test main harness_draw::` → `6 passed; 0 failed` (AC-1..AC-4: uneven 5:1:1 pool uniformity, `Limited` / `>= 98%` `weekly_opus` / missing pool exclusion, partial-pool inclusion, all-limited connected-pool queueing, 0-account 409, caller-task inheritance vs `auto` / Superagent draw, `auto` automation runs and `PUT` preservation, analytics JSON/CSV, delete cleanup).
  - `cargo test -p yulius --test main ui_contract::` → `12 passed; 0 failed`.
  - `cargo clippy --workspace --all-targets -- -D warnings` → passed with 0 warnings.
  - `scripts/dev quick` → passed (`check_repository.py` + `cargo fmt --all -- --check`).
- **Browser end-to-end verification**:
  - `cd e2e && E2E_TELEMETRY_DISABLED=1 npx e2e run tests/antigravity.e2e.ts tests/codex.e2e.ts tests/automations.e2e.ts` → `3 test files passed, 38 passed`.
  - Post-merge (`origin/dev` `57849bf87` and `68ca1fdd2` SPEC-521 #1439) + post-review: `tests/harness-comparison.e2e.ts`, `tests/antigravity.e2e.ts`, `tests/codex.e2e.ts`, `tests/superagent-requests.e2e.ts`, and `tests/model-choices.e2e.ts` (including `Random` effort-tier restriction `['Default', 'Low', 'Medium', 'High']` and `Max` → `Random` effort reset) all passed.
- **Mutation testing** (`scripts/dev mutants --base origin/dev`):
  - `mutants: 108 listed, 38 caught, 0 missed, 0 timed out, 70 unviable, 0 not reached (bound 30 min, took 1032 s)`
- **Security scan** (`scripts/dev security`):
  - Exited 2 (not run, Vidoc off in Yulius for this project/instance; nothing sent).
- **Independent code review**:
  - Round 1 (`yulius-reviewer` on `5cac9313273b2c7cb19a7ab40d9cb2680c6f3ed0`): `Verdict: approve` with 0 blockers/majors/minors and 3 nits (`docs/decisions/README.md` numeric ordering after `origin/dev` merge, stale comment in `crates/yulius/tests/automations.rs:328`, and cross-sandbox `auto` fallback in `mock.js:3863`), all fixed in `0dfa61433`.
  - Round 2 (`yulius-reviewer` on `3f08d498b8fc8c93587cfc76985297b5dfa85a50` after resolving merge with SPEC-521 #1439): `Verdict: approve` with 1 minor test-only item (`e2e/tests/model-choices.e2e.ts` direct assertion on `Random`'s `STANDARD` effort menu and `Max` reset), added with `0` product lines changed since `reviewed_head`.
- **Untested live limits**:
  - Live Anthropic, OpenAI/Codex, and Google/Antigravity OAuth/usage API responses were exercised through synthetic `AgentAccountRecord` fixtures and the mock harness rather than live external provider accounts.
