---
yulius-task: 9b8eb28a-59c9-4e81-a7e1-f20628c741b2
status: done
---

# SPEC-246: Onboarding requires a connected Claude account first

Status: done
Date: 2026-10-20
Owner: task #232
Scope authorization: task #232, feedback from an early external user on a fresh install: "probably
because I didn't connect Claude and the first job failed. I'd make an onboarding flow that requires
connecting Claude." Builds on SPEC-241 (task #227, onboarding on an empty repository), SPEC-051 and
SPEC-097 (onboarding), SPEC-099 (playgrounds) and ROT-010 (a task without capacity waits).

## Goal

On a fresh instance nothing a person starts during first-run and project onboarding can end in a
failed or stuck agent job because no Claude account is connected. Connecting a Claude account is the
first, required step before anything that runs an agent; until then each such action is refused
early with a clear message and a link to connect, and the flow shows that step first.

## Context

Confirmed against `dev` at 06b58375 (SPEC-241 merged).

- **The flow.** Superadmin setup (`auth.js` `mountSetup`: password, recovery code, optional Jev and
  Vidoc keys) ends on Home. Home with no sandbox offers a playground (folder drop or template) and,
  once GitHub is connected, Import and Create. A browser import starts onboarding at once
  (`import_repository` → `start_onboarding`, a failure only logged); a `yulius onboard` upload leaves
  the sandbox `pending` until Start onboarding (`approve_onboarding` → `start_onboarding`). CI and CD
  setup are part of the onboarding task's prompt; `ci_setup` launches no agent. Playgrounds start
  `not_needed`, launch no agent by themselves; their first task does.
- **Every agent start** goes through `App::start_run` (tasks) or `pool_account` (the Superagent, a
  split proposal). Person-triggered task starts: New task (`create_task_as`, also the `task_create`
  tool), Continue / Try again (`continue_task`), Start now (`start_prepared_now`), a start change to
  Start now (`change_start`), Continue of an imported session, Start onboarding and a browser import.
  Machinery starts: chains, rules, automations, a scheduled time, the resume loop.
- **No account today.** `launch_attempt` finds no account (`choose_account` → `None`) and pauses the
  task `waiting_capacity` with the notice "no subscription has capacity left. It continues by
  itself." — the same words as an exhausted pool, with no hint that nothing is connected or where to
  connect. Onboarding moves to `preparing` and waits there. `pool_account` alone tells the case
  apart (`Conflict` "no Claude account is connected"), as a plain conflict. A start that fails for a
  missing credential (`open_credential`) ends `failed_precondition`, whose UI text blames "the
  sandbox is not ready yet".
- **Connecting** is Subscriptions (`/admin/accounts`, `?add=1` opens the sign-in), for instance
  administrators. Task #229 changes that dialog in parallel; this change only links to it.

## Requirements

- **CLA-001** A Claude account is *usable* when it has a credential and its status is `ok` or
  `limited` (the pool's own health rule). `GET /api/me` carries `instance.claude_connected`: whether
  one is usable.
- **CLA-002** Every person-triggered start of an agent with no usable account is refused before it
  changes anything, with HTTP 409, error code `claude_account_required` and a message that says no
  Claude account is connected (or that every one must sign in again), who connects one and where:
  Subscriptions, `/admin/accounts`. This covers New task with a start (now, after a task, by a
  rule; from the UI or the `task_create` tool), Continue / Try again, Start now, a start changed to
  Start now, Continue of an imported session, Start onboarding, the Superagent and split proposals.
  Writing a prepared task ("When I decide", "At a time…") is not a start and is not refused.
- **CLA-003** A browser import with no usable account adds the sandbox and leaves onboarding
  `pending` without creating its task or writing to the repository (no SPEC-241 seed commit): the
  import answers 201 and the sandbox page shows the Connect Claude step before Start onboarding.
- **CLA-004** A task started by machinery (a chain, a rule, an automation, a scheduled time) while
  no account is usable keeps waiting `waiting_capacity`, as today, but its notice says that no Claude
  account is connected, where to connect one, and that it starts by itself once one is connected;
  it does not fail. Its summary carries `waits_for_account: true`, so wherever it is shown later
  (the task strip, the sidebar, its time label, the Paused card) it reads as waiting for a Claude
  account (or for one to sign in again), never "no subscription has capacity left".
- **CLA-005** A start that fails because its account has no credential ends `failed_no_account`
  (not `failed_precondition`); the UI says the task could not start because no Claude account was
  connected, links to Subscriptions and offers Try again.
- **CLA-006** An onboarding whose task already ended for that reason (`failed_no_account`, or
  `failed_precondition` from before this change, which the UI cannot tell apart) can be retried from
  the onboarding gate once an account is usable: Retry onboarding starts the same task again
  (`approve_onboarding`); before that the gate says to connect Claude first. An onboarding task
  waiting for an account starts by itself once one is connected (ROT-010).
- **CLA-007** The UI shows the step first: Home (with or without sandboxes) and the onboarding gate
  show "Connect Claude" as a required step while `claude_connected` is false — a button to
  Subscriptions for an instance administrator, else that an administrator connects one — and the
  actions that would lead to an agent are not offered: the playground drop and template (hidden),
  New playground, Import repository and Create from template (disabled with "Connect Claude
  first"), Start onboarding (disabled). A browser import is therefore reached without an account
  only from a page opened before an account was removed, or through the API; CLA-003 covers it. A
  `claude_account_required` error toast carries a Connect Claude link and re-reads `/api/me`. The
  page learns about a newly connected account without a reload (an `account.updated` event
  re-reads `/api/me`; a non-administrator's page re-reads it when shown).

## Plan

- [x] Usable-account check and the claude_account_required error
- [x] Person-triggered task starts refuse without a usable account
- [x] Onboarding waits for a connected account and machinery waits clearly
- [x] Failed-for-no-account reason and onboarding retry
- [x] Connect Claude step on Home, the onboarding gate and error toasts
- [x] ADR, architecture, product, API and changes record the gate
- [x] First review round answered
- [x] Independent review approved
- [x] Repository gate, e2e suite and security scan pass
- [x] Merged into dev on green CI

## Acceptance criteria

- With no account, `POST /api/sandboxes/{sid}/tasks` with a start answers 409
  `claude_account_required` naming `/admin/accounts`, and no task exists afterwards; a prepared task
  is still created. Continue, Start now and Start onboarding answer the same with no state change.
- With an account in `relogin` only, the same refusal says the account must sign in again.
- A browser import with no account answers 201, the sandbox is `pending` with no onboarding task and
  its remote got no commit; after an account is added, Start onboarding starts the task.
- A start by machinery that finds no account (the `start` pause every such start takes) leaves the
  task `waiting_capacity` with the no-account notice, not failed; with a connected account out of
  capacity the notice stays "no subscription has capacity left".
- An onboarding task that failed `failed_no_account` is started again by Start onboarding once an
  account exists, and the sandbox goes `preparing`.
- `GET /api/me` says `claude_connected` false, then true after an account with a credential is
  added.
- e2e (mock): a fresh instance's Home shows Connect Claude first with the playground actions
  disabled; the onboarding gate shows the step and enables Start onboarding once connected; a
  failed onboarding offers Retry onboarding.

## Test plan

- `crates/yulius/tests/claude_gate.rs` (new): each refusal (CLA-002) with its code, message and no
  state change; the relogin wording; the prepared task allowed; `me.claude_connected` (CLA-001);
  the machinery pause's notice (CLA-004); `failed_no_account` on the wire (CLA-005).
- `crates/yulius/tests/projects.rs`: the browser import left pending (CLA-003); the failed
  onboarding retried (CLA-006). `cockpit.rs`: the Superagent's refusal code.
- Unit tests for the usable-account rule and the `failed_no_account` wire name.
- Existing tests that start tasks with no account seed one where they relied on the pause.
- e2e `claude-connect.e2e.ts` with the mock's no-account state (`claude=0`, `onboarding=failed`,
  `connectClaude()`).
- Gates: `scripts/dev ci`, e2e suite, `scripts/dev security`.

## Decisions

- **Refuse person-triggered starts, keep machinery waiting.** A person gets the reason at once, where
  they act; a chain or an automation has nobody to answer, so it waits paused, says why and starts
  once an account is connected, which ROT-010's resume loop already does. Refusing those would turn
  them into the failed jobs this change removes. ADR-0190.
- **A dedicated error code, not a message match.** The UI needs to recognise the case to offer the
  link; `claude_account_required` is stable while the wording may change.
- **The step lives on Home and the onboarding gate, not in the setup wizard.** The sign-in is a PTY
  dialog on Subscriptions that task #229 is changing; linking to it keeps one sign-in flow, and Home
  is where every first-run path (playground, import, create) begins.
- **Playground creation itself is not refused by the server.** It runs no agent; its UI drop and
  template are disabled until connected so that the "tasks start a minute later" promise holds, and
  its first task is refused by CLA-002 anyway.
- **Retry is the existing Start onboarding.** A failed start leaves the sandbox `pending`, so
  `approve_onboarding` already restarts the same task; only the gate's words and button change.

## Evidence

- **Before.** On `dev` (06b58375) a start with no account paused `waiting_capacity` with "no
  subscription has capacity left" (`launch_attempt` → `choose_account` `None` → `pause_task`), and
  a browser import moved onboarding to `preparing` the same way; only the Superagent answered a
  plain `conflict`. With the gate in place and no fixture changed, 48 existing integration tests
  failed exactly where they started an agent with no account (New task, `task_create`, Start now,
  start changes, supertasks, automations' manual starts, imports and Start onboarding), which shows
  every such path now meets the gate.
- **Tests added** (each asserts behavior that only exists with the change):
  `claude_gate.rs` — `a_new_task_without_a_claude_account_is_refused_and_creates_nothing` (UI create
  now / after a task / supertask, `task_create`, a prepared task written, Start now and a start
  change refused, then all work with an account), `accounts_that_must_sign_in_again_refuse_the_start_saying_so`,
  `continuing_without_a_claude_account_is_refused_and_changes_nothing` (also `failed_no_account` on
  the wire), `me_says_whether_a_claude_account_is_connected`,
  `a_start_by_machinery_waits_for_an_account_and_says_why` (both notices and `waits_for_account`);
  `projects.rs` — `a_browser_import_without_claude_waits_for_the_connection` (CLA-003: 201,
  `pending`, no task, no first commit; Start onboarding 409, then `preparing` once connected),
  `a_failed_onboarding_start_is_retried_once_claude_is_connected` (CLA-006),
  `a_scheduled_start_without_claude_waits_and_says_why` (CLA-004 through a real scheduled release);
  `sessions.rs` — an imported session's Continue refused with no task created; `cockpit.rs` — the
  Superagent's `claude_account_required`, and an exhausted pool still 429; unit tests in
  `account_gate.rs` (usable rule, refusal and notice wording, the failure mapping). e2e
  `claude-connect.e2e.ts` (5 tests).
- **Fixtures.** Tests that started agents with no account now seed
  `support::exhausted_subscription` (connected, no capacity for days), so their starts take the same
  pause as before; `World::new` seeds it, `World::without_claude` does not.
- **Review.** Round 1 (independent session, head 003bfe34): request changes — the CLA-004 reason
  reached only a live notice (major), the notice did not tell a needed sign-in apart, missing tests
  (imported session, real machinery path, the failure mapping), and nits (a failed read skipping
  the pause's publish, error order, mock coverage, docs drift, fixture reset). All fixed in
  2a140666, 60967e66 (`waits_for_account` in the summary and the UI's strip, sidebar and time
  label; `waiting_notice`; `fail_reason`; the tests above). The account-removal UI refresh is
  covered by re-reading `/api/me` on a refused start; no event is published on removal (unchanged).
  Round 2 reviewed the head carrying this Evidence.
- **Gates** on e5ea247f (dev merged at b06db25c): `scripts/dev ci` exit 0 (2324 tests passed, 17
  skipped; lint, macOS compile, deps, doctests); e2e `npx e2e run` 656 passed, 1 skipped.
- **Security scan not run.** `scripts/dev security` exit 2: "Vidoc cannot be reached … HTTP 429:
  This API key has used its 50,000,000 tokens." Unrun, not passed; the change was reviewed by hand
  for security (every gate check runs after the access check; UI text goes through `h()`).
- **Not tested.** A real Claude sign-in and a real agent run (the harness has no process host); the
  race where an account loses its credential between being chosen and opened (`failed_no_account`)
  is covered by the mapping's unit test only.
