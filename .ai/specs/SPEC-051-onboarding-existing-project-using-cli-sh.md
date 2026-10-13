---
yulius-task: b6ac9feb-ab64-4643-bcdd-779dd524c985
status: in-progress
---

# SPEC-051: onboarding through the CLI uploads; a person starts it in the browser

Status: in progress
Date: 2026-10-13
Owner: Yulius task #43
Scope authorization: user request: "Onboarding existing project using cli should upload things
etc but running adjusting the repo to the standards should require explicit action in the GUI
"Start onboarding" or something, never via the CLI".

## Goal

`yulius onboard` adds the repository (when it is not on Yulius yet) and uploads the person's
branches, snapshot and Claude Code sessions, as today. It no longer starts the onboarding task
that adapts the repository to the Yulius standards. That task starts only when a person clicks
**Start onboarding** on the sandbox page in the browser. The CLI tells them so and where.

## Context

- Onboarding lives in `crates/yulius-app/src/onboarding.rs` (SPEC-001 DSC-010, IMP-005, IMP-010,
  U-08; SPEC-002 §6.5, §6.6). Every imported repository starts `pending` with a resting onboarding
  task: the full preparation ("Prepare this repository for Yulius") or the environment-only task.
- `yulius onboard` creates the sandbox through `POST /api/onboard/jobs`
  (`import_repository(.., start_preparation: false)`), and the job's apply step
  (`crates/yulius-app/src/imports.rs`, `apply_import`) then calls `start_onboarding`: the task
  starts by itself once the upload lands (F 2026-10-10, "no approval step"). That is what the
  request removes.
- Adding a repository in the web UI (`POST /api/sandboxes` mode `import`) starts the task at once
  (`start_preparation: true`). That click is already an explicit GUI action; this spec keeps it.
- `POST /api/sandboxes/{sid}/onboarding/approve` (write) starts a `pending` sandbox's task. The
  sandbox page shows a banner with **Start preparation** (write) and **Skip for now** (admin) while
  the sandbox is `pending` (`crates/yulius-server/web/assets/taskview.js`). Any API caller can use
  the endpoint today, including the CLI's bearer token (`yulius login`).
- `AuthContext.csrf_token` is `Some` exactly for a browser cookie session and `None` for a bearer
  token (CLI, scripts).
- `App::onboarding_status` writes the summary and the pending items the CLI prints
  (`crates/yulius-import/src/run.rs`, `show_onboarding`). For `pending` it says the task "starts
  when the upload finishes".
- Related: task #34 (SPEC-036) verifies the onboarding docs against the code; this change updates
  the same documents, so it merges `dev` before delivery.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OBD-001 | Applying a `yulius onboard` upload never starts the onboarding task. A `pending` sandbox stays `pending` with its resting task. The import result reports `onboarding: "pending"`. Branches, the snapshot and the sessions land as before. | Rust `import_server::import_resumes_applies_and_reruns_idempotently` |
| OBD-002 | `POST /api/sandboxes/{sid}/onboarding/approve` starts onboarding only from a browser session. A bearer token (the CLI, scripts) gets 403 `forbidden` ("Start onboarding on the sandbox page in the browser") and nothing changes. A browser session with write access starts it as before (`pending → preparing`). | Rust `projects::onboarding_starts_only_from_the_browser` |
| OBD-003 | While a sandbox is `pending`, its onboarding status says a person starts it in the browser: the summary names the task and the pending item reads "someone clicks Start onboarding on the sandbox page: <sandbox URL>". `yulius onboard` prints it, and says that new tasks wait. | Rust `import_server::*` (status text), `yulius-import` `run::tests` |
| OBD-004 | The sandbox page's banner for a `pending` sandbox says "This sandbox needs onboarding", explains that onboarding adapts the repository to the Yulius standards, and offers **Start onboarding** (write access) and **Skip for now** (admin). The home page badge reads "Needs onboarding". | e2e `onboarding.e2e.ts` |
| OBD-005 | Adding a repository in the web UI still starts its onboarding task at once (unchanged), and new tasks still wait after a `pending` or `preparing` sandbox's onboarding task (IMP-010, unchanged). | Rust `projects::*` (existing) |

## Acceptance criteria

- AC-OBD-1: `yulius onboard` of a new repository uploads everything; afterwards the sandbox is
  `pending`, its onboarding task has not run, and the CLI says to click Start onboarding on the
  sandbox page with its URL.
- AC-OBD-2: calling the approve endpoint with the CLI's token is refused with 403 and the sandbox
  stays `pending`.
- AC-OBD-3: clicking Start onboarding on the sandbox page starts the task; the sandbox becomes
  `preparing` and the banner says it is being prepared.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [ ] An upload through yulius onboard leaves onboarding waiting for a person
- [ ] Only a browser session can start onboarding
- [ ] The CLI and the onboarding status say where to start onboarding
- [ ] The sandbox page offers Start onboarding, covered end to end
- [ ] Independent review in a fresh session, findings fixed test-first
- [ ] Every gate green, docs and the changes log updated

## Design

- `apply_import` drops its `start_onboarding` call; the rest of the apply is unchanged. The
  sandbox's onboarding task already exists (created when the job linked the repository); a
  sandbox without one gets it from `start_onboarding` when the person clicks the button.
- `App::approve_onboarding` refuses a context without `csrf_token` with `Forbidden` before it
  reads the sandbox state, so a CLI token can neither start nor probe it. The cookie session
  already passes the CSRF check in the server's middleware.
- `onboarding_status` for `pending`: summary "Not ready yet: task #N waits for someone to start
  onboarding." and pending items "someone clicks Start onboarding on the sandbox page:
  {public_url}/s/{slug}" and "the repository passes the Yulius format check".
- `show_onboarding` in the CLI keeps printing the summary and pending items; its closing line for
  a `pending` sandbox reads "New tasks wait and start by themselves once onboarding is ready."
  (unchanged wording for other states).
- UI copy only: the banner and the button in `taskview.js`, the badge in `pages/home.js`, the mock
  route and the web README.

## Test plan

- Integration (`crates/yulius/tests/import_server.rs`): after the upload is applied the result is
  `onboarding: "pending"`, the stored sandbox is `pending`, the onboarding task has no attempt, and
  the status's pending items name "Start onboarding" and the sandbox URL. Fails today (the task
  starts and the result says `preparing`).
- Integration (`crates/yulius/tests/projects.rs`): a bearer token gets 403 from approve and the
  sandbox stays `pending`; a browser session (cookie + CSRF) of the same user starts it.
  `World::browser` creates the session in the store. Fails today (bearer is accepted).
- Unit (`crates/yulius-import/src/run.rs`): a `pending` status is printed with its pending items.
- End to end (`e2e/tests/onboarding.e2e.ts`, mock backend): importing a repository shows the
  banner with Start onboarding; clicking it shows "Preparing this sandbox".
- Failure cases: approve with a bearer token (403), approve with nothing pending (409, existing),
  approve while preparing (idempotent, existing).

## Decisions

- Decided: the server refuses a bearer token on approve, not only the CLI's code path. "Never via
  the CLI" then holds for any client holding a CLI token, not just today's `yulius onboard`.
  ADR-0038.
- Assumed (reversible): adding a repository in the web UI keeps starting onboarding at once,
  because that click is itself the explicit GUI action; the request is about the CLI path.
- Assumed (reversible): the button is named "Start onboarding" (the request's wording) instead of
  "Start preparation"; the task's title stays "Prepare this repository for Yulius".
- Assumed (reversible): no new endpoint or state. `pending` already means "waits for a person"
  and the approve endpoint already starts it.
- Not now: a CLI flag to start onboarding (explicitly out of scope: "never via the CLI").
