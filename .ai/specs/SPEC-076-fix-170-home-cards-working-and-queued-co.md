---
yulius-task: 233afd33-bd2e-4803-9d30-ae083e8481ed
status: in-progress
---

# SPEC-076: Home cards' working and queued counts follow the sidebar's rule

Status: in progress
Date: 2026-10-13
Owner: Yulius task #62
Scope authorization: GitHub issue #170, through task #62 (part of task #47, "fix all open
actionable GitHub issues"). No new GitHub issue is created.

## Goal

A sandbox's three counters on Home's cards and the sandbox tabs split the sidebar's groups with
the sidebar's own rule: "needs you" is the Needs you group, and "working" plus "queued" is the
Working group, "queued" being its queued and chained tasks. The server decides every task's group
with one rule, the one `taskInfo` applies in the web UI.

## Context

- #147/#167 made "needs you" one rule: `AttentionFacts::needs_you` in `yulius-domain`
  (`task.rs`), mirrored by `taskInfo` in `web/assets/status.js`, counted by
  `App::sandbox_view` (`yulius-app/src/sandboxes.rs`) into `SandboxCounts.needs_attention`.
- `SandboxCounts.running` still counts tasks with a live process (`TaskStatus::is_live`:
  starting, running, switching, stopping) and `queued` only `Queued` tasks. The sidebar's Working
  group (`statusInfo`/`taskInfo` group `working`) instead holds running tasks that are working,
  a finished turn kept going by background shells, a pull request in review, and queued, starting,
  switching, paused (`waiting_capacity`) and unblocked chained tasks. It leaves out "Your turn",
  permission prompts, Done tasks and Stopping ones (grouped with the ended). So a card can say
  "5 working" beside a sidebar "Working 6" (#170).
- `AttentionFacts::needs_you` returns early only for resting tasks (closed, stopped, failed): a
  Stopping task with a held prompt counts as needing you on the server, while the sidebar lists it
  as ended. The same disagreement in miniature.
- Home (`pages/home.js`) shows `counts.running` as "working" and `counts.queued` as "queued"; the
  tabs (`layout.js`) show `counts.running` as the working number. The mock (`mock.js` `recount`)
  copies the server's live-process rule for `running`.
- The status page's `live_sessions` counts live processes separately, so the sandbox counters do
  not need to.

## Requirements

- R1. One domain rule gives each task its sidebar group: Needs you, Working, Queued (the queued
  part of the Working group), Done or Ended, exactly as `taskInfo` does. `needs_you` is that rule's
  Needs you group.
- R2. `counts.needs_attention`, `counts.running` and `counts.queued` count the tasks of the Needs
  you, Working (less queued) and Queued groups. Queued counts `queued` and unblocked `chained`
  tasks; a chain whose predecessor ended otherwise needs you. Working counts running (working, or
  kept going by background shells, or in review), starting, switching and paused tasks. Stopping,
  closed, stopped and failed tasks count nowhere, nor do Done ones.
- R3. The wire field names stay (`running`, `needs_attention`, `queued`); their documentation says
  the rule.
- R4. The mock counts the same way from `taskInfo`, so the e2e suite checks the UI rule end to
  end.
- R5. SPEC-001's UI rows (UI-045, UI-050, AC-UI-45) and `docs/api.md` state the rule.

## Plan

- [x] The task's spec and plan.
- [ ] One domain rule groups a task as the sidebar does, with unit tests.
- [ ] The sandbox counters count by that rule, with an integration test.
- [ ] Home, the tabs and the mock count working and queued by the sidebar's groups, with an e2e
      test.
- [ ] SPEC-001, the API document, an ADR and the changes say the rule.
- [ ] Independent review, gates green and delivery into `dev`.

## Acceptance criteria

- AC1. For any state, a sandbox card's working + queued equals its sidebar Working count and its
  "needs you" equals the Needs you count (mock e2e; server integration test with queued, chained,
  paused, starting, running, Your turn, Done, in-review and stopping tasks).
- AC2. A Stopping task with a held prompt does not need you (unit test).
- AC3. SPEC-001's UI-045, UI-050 and AC-UI-45 rows state the rule.

## Test plan

- Unit (`yulius-domain` `task.rs`): a table of statuses, activities and facts to groups, covering
  every status and the pull-request branches of a finished turn; `needs_you` agrees with
  `group() == NeedsYou`.
- Integration (`crates/yulius/tests/delivery.rs`): tasks in each group in one sandbox;
  `GET /api/sandboxes` counters equal the expected groups, before and after the Git overview reads
  the pull request (a finished turn with checks running is Working, In review).
- E2e (`e2e/tests/consistency.e2e.ts`): on `?mock=1`, each sandbox's card working + queued equals
  its sidebar Working count, its tab says the same working number, and "needs you" agrees.

## Decisions

- Keep the wire names and change what `running` counts, rather than renaming it `working`: the
  only readers are the web UI and the mock in this repository, the name stays readable as "the
  running part of the work", and a rename would break any script reading the field for no user
  gain. Recorded in ADR-0052.
- The rule lives in the domain next to `needs_you` (one `TaskGroup` from `AttentionFacts`) rather
  than as a second function in the app: the sidebar's rule is one decision tree, and keeping it in
  one place is the point of #170. `AttentionFacts` keeps its name: it is the facts of a task's
  sidebar state, and renaming it widens the diff for no behavior.
- A Stopping task belongs to Ended on the server too (the sidebar's rule), so a held prompt on a
  task being stopped no longer counts as needing you.
