---
yulius-task: b6ac9feb-ab64-4643-bcdd-779dd524c985
status: done
---

# SPEC-051: onboarding through the CLI uploads; a person starts it in the browser

Status: implemented
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
  task (since SPEC-097 the task is created only when onboarding starts or a task waits for it): the full preparation ("Prepare this repository for Yulius") or the environment-only task.
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
| OBD-001 | Applying a `yulius onboard` upload never starts the onboarding task. A `pending` sandbox stays `pending` (since SPEC-097 with no onboarding task until onboarding starts or a task waits for it). The import result reports `onboarding: "pending"`. Branches, the snapshot and the sessions land as before. | Rust `import_server::import_resumes_applies_and_reruns_idempotently` |
| OBD-002 | Only a person in the browser starts onboarding (amended by SPEC-247 RMT-021, ADR-0188: also a person's own agent with an `operate` token made on Settings; `yulius login` tokens stay refused). `POST /api/sandboxes/{sid}/onboarding/approve` (Start onboarding) and `POST /api/tasks/{tid}/continue` on a `pending` sandbox's onboarding task refuse a bearer token (the CLI, scripts) with 403 `forbidden` ("Start onboarding on the sandbox page, or run `yulius project onboard` with an operate token …", SPEC-313) and change nothing. A browser session with write access starts it as before (`pending → preparing`). | Rust `import_server::onboarding_starts_only_from_the_browser` |
| OBD-003 | While a sandbox is `pending`, its onboarding status says how a person starts it: the summary names the task ("the preparation task" while it has none, SPEC-097) and the pending item reads "someone starts onboarding: their agent runs `yulius project onboard` (`onboarding_start`) with an operate token, or they click Start onboarding on the sandbox page: <sandbox URL>" (amended by SPEC-313 for ADR-0188; it read "someone clicks Start onboarding on the sandbox page: <sandbox URL>"). `yulius onboard` prints it ("● waiting to start: Start onboarding on the sandbox page, or `yulius project onboard` with an operate token"; it read "● waiting for Start onboarding in the browser"), and says that new tasks wait. | Rust `import_server::import_resumes_applies_and_reruns_idempotently` (status text), `yulius-import` `end_to_end::onboards_everything_then_sends_nothing_new_on_rerun`, `end_to_end::dry_runs_status_and_declined_confirmations_upload_nothing` |
| OBD-004 | The sandbox page's banner for a `pending` sandbox says "This sandbox needs onboarding", explains that onboarding adapts the repository to the Yulius standards, and offers **Start onboarding** (write access) and **Skip for now** (admin). The home page badge reads "Needs onboarding". Superseded on the page by SPEC-096 (OBG-001..004): the page shows only what onboarding will do and Start onboarding, no task input and no Skip; the badge is unchanged. | e2e `onboarding.e2e.ts` |
| OBD-005 | Adding a repository in the web UI (a browser session) still starts its onboarding task at once; the same request with a bearer token leaves the sandbox `pending` like `yulius onboard`. New tasks still wait after a `pending` or `preparing` sandbox's onboarding task (IMP-010, unchanged). | Rust `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding`, `sessions::no_task_starts_before_onboarding_is_ready_and_the_plan_says_why` |

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
- [x] An upload through yulius onboard leaves onboarding waiting for a person
- [x] Only a browser session can start onboarding
- [x] The CLI and the onboarding status say where to start onboarding
- [x] The sandbox page offers Start onboarding, covered end to end
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Docs, ADR and the changes log say onboarding starts in the browser
- [x] Every gate green on the reviewed head, evidence recorded

## Design

- `apply_import` drops its `start_onboarding` call; the rest of the apply is unchanged. The
  sandbox's onboarding task already exists (created when the job linked the repository); a
  sandbox without one gets it from `start_onboarding` when the person clicks the button.
- `AuthContext::is_browser()` is true exactly for a cookie session (it carries a CSRF token,
  which the server's middleware checks on every POST); a bearer token is not a browser.
- `App::approve_onboarding` refuses a non-browser caller with `Forbidden` before it reads the
  sandbox state, so a CLI token can neither start nor probe it.
- `continue_task` refuses a non-browser caller when the task is the onboarding task of a `pending`
  sandbox (continuing it would start onboarding); other tasks are unaffected.
- `create_sandbox` (mode `import`) starts onboarding at once only for a browser caller; a token
  import stays `pending`.
- `onboarding_status` for `pending`: summary "Not ready yet: task #N waits for someone to start
  onboarding." and pending items "someone clicks Start onboarding on the sandbox page:
  {public_url}/s/{slug}" and "the repository passes the Yulius format check".
- `show_onboarding` in the CLI labels a `pending` sandbox "● waiting for Start onboarding in the
  browser" and keeps printing the summary, the pending items and the closing line ("New tasks
  wait for it and start by themselves once onboarding is ready."). SPEC-313 rewords both, and
  the refusal, to name `yulius project onboard` with an operate token too (ADR-0188).
- UI copy only: the banner and the button in `taskview.js`, the badge in `pages/home.js`, the mock
  route and the web README.

## Test plan

- Integration (`crates/yulius/tests/import_server.rs`): after the upload is applied the result is
  `onboarding: "pending"`, the stored sandbox is `pending`, the onboarding task has no attempt, and
  the status's pending items name "Start onboarding" and the sandbox URL. Fails today (the task
  starts and the result says `preparing`).
- Integration (`crates/yulius/tests/import_server.rs`, `onboarding_starts_only_from_the_browser`):
  a bearer token gets 403 from approve and from continuing the onboarding task, the sandbox stays
  `pending` and the task has no attempt; a browser session (cookie + CSRF) of the same user starts
  it. `World::browser` creates the session in the store. Failed before the change (bearer was
  accepted on both).
- Integration (`crates/yulius/tests/projects.rs`): a browser import starts onboarding at once, a
  bearer import stays `pending`.
- CLI end to end (`crates/yulius-import/tests/import/end_to_end.rs`, fake server answering like
  the real one): a `pending` status is printed with its label and pending items.
- End to end (`e2e/tests/onboarding.e2e.ts`, mock backend): a `pending` sandbox shows "Needs
  onboarding" on Home and the banner with Start onboarding; clicking it shows "Preparing this
  sandbox". The mock's import leaves a sandbox `pending` and stands in for the CLI path.
- Failure cases: approve or continue with a bearer token (403), approve with nothing pending (409, existing),
  approve while preparing (idempotent, existing).

## Decisions

- Decided: the server refuses a bearer token on approve, not only the CLI's code path. "Never via
  the CLI" then holds for any client holding a CLI token, not just today's `yulius onboard`.
  ADR-0038.
- Assumed (reversible): adding a repository in the web UI keeps starting onboarding at once,
  because that click is itself the explicit GUI action; the server keys this on the browser
  session, not the route, so a token import waits like the CLI (review round 1).
- Decided: ADR number 0038 skips 0033-0037 because parallel tasks still renumber their 0030
  records into that range.
- Assumed (reversible): the button is named "Start onboarding" (the request's wording) instead of
  "Start preparation"; the task's title stays "Prepare this repository for Yulius".
- Assumed (reversible): no new endpoint or state. `pending` already means "waits for a person"
  and the approve endpoint already starts it.
- Not now: a CLI flag to start onboarding (explicitly out of scope: "never via the CLI").

## Evidence

Head 87d3e8e (the reviewed fixes plus `origin/dev` merged), 2026-10-13:

- `CARGO_BUILD_JOBS=2 scripts/dev ci`: exit 0. Repository checks, `cargo check`, fmt, clippy
  `-D warnings`, nextest (1219 tests run: 1219 passed, 16 skipped), doctests, `cargo deny`,
  `cargo machete`, `cargo doc`. Earlier runs at full parallelism were killed by the system
  (rustc SIGKILL at a load average of about 40 on this shared machine), not by a check.
- Test-first: `import_server::import_resumes_applies_and_reruns_idempotently` failed with
  `onboarding: "preparing"` before OBD-001. `import_server::onboarding_starts_only_from_the_browser`
  failed with 200 instead of 403, first for approve, then (review round 1) for continue.
  `projects::import_links_owner_repositories_with_profiles_ports_and_onboarding` failed with
  `preparing` for a bearer import. The CLI's `end_to_end` tests failed on the old label. All pass.
- `cd e2e && npx e2e run`: `onboarding.e2e.ts` (2 tests, including the new Start onboarding test,
  axe clean), `home`, `quality` and `responsive` pass. Under that load 20 tests in unrelated files
  timed out. Rerunning those 9 files left one failure (`new-task.e2e.ts`, a transient "Choosing
  mode and workflow…" text), and it passes when run alone.
- Independent review (fresh read-only subagent, `om-code-review`): round 1 requested changes
  (a bearer token could still start onboarding through `POST /api/tasks/{tid}/continue` and through
  `POST /api/sandboxes` import), fixed test-first in 51758b8; round 2 approved 51758b8, and its two
  nits (assert the 403 message, ADR note on continuing after start) are in 87d3e8e.
- Not tested here: a real `yulius onboard` against a deployed server and a real browser click on
  production. The flows are covered by the integration tests, the CLI's fake-server end-to-end
  tests and the mock-backed browser test.
