---
yulius-task: da6c0cac-674c-45da-89f1-36accddda52c
status: in-progress
---

# SPEC-096: onboarding comes first: the new-task page shows only what it will do and its button

Status: in progress
Date: 2026-10-14
Owner: Yulius task #81
Scope authorization: task request: "onboarding should be mandatory and should show no task input
until finished. Just centered vertically and horizontally message with button, describing what
exactly will happen", with a screenshot of today's page.

## Goal

A person who opens a sandbox that still needs onboarding sees one thing, centred on the page: what
onboarding will do, step by step, and the button that starts it. There is no task box, no task
choices and no way to skip onboarding from the page. While onboarding runs, the same place says
that it is running and links to its task. The task box appears by itself once the sandbox is ready.

## Context

- Today (screenshot of 2026-10-14) a `pending` sandbox's new-task page shows the brand mark and a
  quote. Below them is a banner, "This sandbox needs onboarding. …", with **Start onboarding** and
  **Skip for now** buttons, and under it the whole composer: the prompt, mode, workflow, start,
  model and effort. While the sandbox is `preparing`, a "Preparing this sandbox." banner shows above
  the same composer (`renderBanner` in `crates/yulius-server/web/assets/taskview.js`).
- Onboarding states (`crates/yulius-app/src/onboarding.rs`): `not_needed | pending → preparing →
  ready | skipped`. `pending` waits for a person with write access to click Start onboarding in the
  browser (SPEC-051, ADR-0038). `preparing` runs the onboarding task. Ready means its agent
  finished and the repository passes the Yulius format check. `POST
  /api/sandboxes/{sid}/onboarding/skip` (admin) ends the wait.
- The onboarding task (`onboarding_prompt`, `environment_prompt`) sets up the sandbox's
  development environment (toolchains, packages, the repository's own checks). It also writes
  what the repository lacks for the Yulius format: the product summary, the architecture document,
  decision records, the specs directory, the changes log, `yulius.toml` and the `CLAUDE.md`
  pointer. Documents that already exist are mapped instead of moved. GitHub Actions jobs move into
  Yulius CI. It changes documentation, configuration and CI only, never application code, and its
  work reaches the integration branch through a pull request that Yulius opens.
- Tasks created meanwhile through the API, CLI, MCP or automations are not refused: they wait
  after the onboarding task and start once the sandbox is ready (U-08, SPEC-001 IMP-010). That
  stays as it is.
- The e2e suite (`e2e/tests/onboarding.e2e.ts`) covers both banners against the mock server
  (`crates/yulius-server/web/assets/mock.js`). The mock's import leaves the sandbox `pending`, and
  its approve route makes it `ready` 9 s later.

## Requirements

| ID | Requirement | Verification |
| --- | --- | --- |
| OBG-001 | While the sandbox is `pending` or `preparing`, the new-task page shows no task input: the prompt, its choices (mode, workflow, start, model, effort), preview, start button, hint, quote and brand mark are not rendered visible. The breadcrumb and repository bar stay. | e2e `onboarding.e2e.ts` |
| OBG-002 | In their place, a region labelled by its heading is centred vertically and horizontally in the page. For `pending` it says what exactly onboarding will do: that it is one task an agent runs before any other (linking it by number and title), that task's own plan (the steps the server gave it, which differ for a repository to adapt and one already in the format), that it changes documentation, configuration and CI only and any change reaches the integration branch through a pull request, and when the task box appears. | e2e: text for both kinds, centring measured from bounding boxes at desktop and phone width |
| OBG-003 | A person with write access sees one button, **Start onboarding**, which starts it (the existing approve call). A reader sees that someone with write access starts it, and no button. | e2e (write); reader covered by the `allows(access, 'write')` branch, as the banner's button was |
| OBG-004 | No **Skip for now** is offered on the page: onboarding is mandatory in the UI. The admin skip endpoint stays as an operator's recovery path (ADR-0076). | e2e: no Skip button in either state |
| OBG-005 | While `preparing`, the region says where onboarding stands, as the server's `onboarding_status` reads it: running (**Follow onboarding**, a link styled as the primary button, opens the task), waiting for a subscription with capacity, stopped before the format check passed (**Open onboarding**), or its task missing (an administrator is told the API recovery path). While the sandbox's tasks are not known (not read yet, or reading them failed) it says onboarding is running, never that its task is missing. After Start onboarding the keyboard moves to its action, and keeps it as task events arrive. | e2e (running, capacity, stopped, missing, tasks unknown) |
| OBG-006 | When the sandbox becomes `ready` (or an operator skips it), the region goes away and the composer shows without a reload; it takes the keyboard when the person was not busy elsewhere, and a polite status says so. | e2e: the mock's `finishOnboarding` |
| OBG-007 | The page has no WCAG violation in either state, at desktop and phone width. The new-task shortcut on a gated sandbox puts the keyboard on its action. | e2e with axe; shortcut in e2e |

## Plan

- [ ] Read the product context and related work, then write and validate the spec
- [ ] A pending sandbox's page shows only the centred onboarding message and its Start button
- [ ] A preparing sandbox shows that onboarding runs and follows it; the composer returns when ready
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Acceptance criteria

- On a `pending` sandbox's new-task page, `Task prompt` and `Start task` are absent or hidden, the
  page shows the onboarding message centred, and it has a **Start onboarding** button and no
  **Skip for now**.
- The plan shown is the onboarding task's: six steps with GitHub Actions for a repository to adapt,
  three without for one already in the Yulius format.
- After Start onboarding, the message says onboarding is running, with **Follow onboarding**
  opening task #1. Once the mock marks the sandbox ready, the prompt shows and takes input.
- axe finds no violation in the pending and preparing states.
- No server change: tasks created through the API still wait after onboarding.

## Test plan

- Rewrite `e2e/tests/onboarding.e2e.ts` first (it fails on today's page, which shows the composer
  and Skip for now):
  - pending: the region's heading and its task's plan, no prompt or start button, no Skip, the
    region's centre within 2 px of the free page area's centre at desktop and 390×844, axe at both,
    and the new-task shortcut focusing Start onboarding.
  - the environment kind (mock `mobile-app`): its three steps, no GitHub Actions.
  - Start onboarding → the running message with Follow onboarding focused, still focused after a
    task event; the capacity and stopped wordings; Follow opens `#/s/website/t/1`; axe; then the
    composer appears, focused and announced, once the test finishes onboarding in the mock
    (`finishOnboarding`, no timer race).
  - The old test where a task started from the composer during preparation waited is replaced:
    the page no longer offers that path. The API path stays covered by the Rust tests (U-08).
- Gates: `scripts/dev ci` and the full e2e suite.

## Decisions

- **Gate the composer, do not refuse tasks on the server.** The request is about the page. The
  server already holds every task created meanwhile until onboarding lands (U-08), and automations,
  the CLI and agents rely on that. Refusing them would break those callers for no gain.
- **No skip in the UI, keep the admin API (ADR-0076).** "Mandatory" means a person is not offered a
  way around it. An onboarding task can still fail beyond repair (a broken clone, a repository it
  cannot validate), and an operator then needs a way out without database edits. The admin-only
  endpoint stays for that, documented as recovery rather than a choice.
- **Hide, do not destroy, the composer.** The composer is built once and hidden while gated, so a
  draft typed earlier (drafts are per sandbox, SPEC-026) and its attachments survive, and the page
  needs no remount when the sandbox becomes ready.
- **The plan shown is the onboarding task's own** (review finding). A fixed text cannot be exact
  for both kinds: the environment-only task neither moves GitHub Actions nor adds documents. The
  task's steps, read from `GET /api/tasks/{tid}`, are what the server planned for this repository,
  so the page needs no new API field and stays true when `onboarding.rs` changes. Around them only
  what holds for both kinds is said.
- **The superadmin's recovery-code reminder waits while gated.** It is part of the composer card.
  Showing it beside the gate would break "just a message with a button", and it returns once the
  sandbox is ready. Other sandboxes still show it.
