---
yulius-task: 3673bc26-a616-43e2-9691-450c90e820c0
status: done
---

# SPEC-578: Actionable Needs You messages, browser handoff lifecycle, and task wait history

Tracked in issue [#1537](https://github.com/Quartio-Studio/yulius/issues/1537).

## Goal

Whenever a task enters **Needs You**, its permitted owner immediately sees an actionable,
redacted explanation of **what happened**, **what to do**, and **where to do it** — beside the
status on the task page (even when the agent has stopped or parked), reachable from the Needs You
list and notifications, persisted as one timestamped message per wait episode in task history, and
returned identically from `tasks_list` and `task_get`.

## Context

Three live incidents on the Quartio instance (20 Nov 2026) exposed four linked root causes:

1. **Browser handoffs disappear from explanations and stay stuck forever (Quartio-v2 #334).**
   - `App::tool_browser_request_user_action` (`crates/yulius-app/src/browser.rs`) records an
     in-memory `ActionRequest` on the browser tab (`BrowserInstance.state.actions`) and sets
     `tasks.attention` (`Activity::NeedsAttention`).
   - As soon as `activity` leaves `NeedsAttention` (e.g. the agent moves on, stops, or parks),
     `App::attention_view` returns `None`, while `tab.action_requested` stays in memory.
   - `App::browser_requests` (`crates/yulius-app/src/sandboxes.rs`) reduces browser requests to a
     bare `HashSet<String>` of task IDs, discarding `reason` and `tab_id`. `App::waits_why`
     (`crates/yulius-app/src/task_questions.rs`) then sees `needs_you: None` and `attention: None`,
     falling back to `"A request for a person was pending when the task list was read."`.
   - `BrowserInstance::clear_action` is never called in production, `BrowserSession` does not expose
     it, `routes/browser.rs` drops `BrowserNotice::ActionCleared`, and `UiEvent` has no
     `browser.action_cleared` event. Answering, resolving, superseding, or closing a task never
     clears the tab handoff, so #334 remained stuck in Needs You even after cleanup.
   - In the web UI, `store.state.actionRequests` is populated only from live WS events or opening
     the Browser panel, so a page refresh with the Browser panel closed loses the handoff.
2. **System-generated stops with open plan steps give no specific step or action (Yulius #540).**
   - `derived_wait_reason` returns only `"The agent stopped with unfinished steps in its plan."`
     without naming the outstanding step(s) or next action.
   - `TaskSummary` carries no derived explanation; `taskview.js` shows generic
     `"The agent is waiting for your reply"` (or generic parked text); `status.js` overwrites all
     parked tasks with `PARKED_LINE` and shows generic `"Agent replied — your turn"`.
   - `notifications.js` and `notify.rs` omit the specific explanation and next action.
3. **Failing PR checks lack failed job/test details and mask agent-recoverable/progressing runs (Yulius #553).**
   - `pr_news_due` (`crates/yulius-app/src/pr_watch.rs`) does not treat an untold failed CI run
     waiting to be delivered to a live unpaused agent — or a newer queued/running CI run on the
     PR — as agent-handled/progressing, causing transient or recoverable failures to show Needs You.
   - When human action genuinely is needed, `derived_wait_reason` omits the PR number, failed job
     name, failing test names (already parsed by `pr_watch::failed_jobs`), CI link, and concrete
     action; `taskview.js` hides `.attention-bar` completely (`&& !info.pr`).
4. **No timestamped wait history or per-episode deduplication.**
   - Entering Needs You records no timestamped system or agent message in task history.

**Scope & coordination:** Coordinated with task #540 (`SPEC-558`), which owns parked-task PR
verification and parking eligibility (`parking.rs`, `delivery.rs` parked checks). This task (#562)
owns the user-visible Needs You reason/message, browser handoff lifecycle, wait-episode history,
notifications, remote MCP consistency, and UI presentation.

## Requirements

- **NYM-001 (Unified Needs You notice):** Every task classified in `TaskGroup::NeedsYou` resolves
  a redacted, bounded `NeedsYouNotice` containing `source` (`agent`, `browser`, `terminal`, or
  `system`), `episode` (stable wait-episode key), `happened`, `problem` (optional), `action`,
  `link` (a page of this Yulius), `decision`, `browser_tab` (optional), `at` (RFC 3339), and
  `at_ms`. Tasks outside `NeedsYou` resolve `None`.
- **NYM-002 (Browser handoff lifecycle & legacy fallback):**
  - `browser_request_user_action` for a task persists a `NeedsYouRecord` (with `browser_tab`,
    redacted reason, concrete browser action, and `/s/<slug>/browser?tab=<tab_id>` link), flags
    `attention`, supersedes any prior browser handoff on another tab for the same task, and emits
    `browser.action_requested` + `task.updated`.
  - Genuine owner browser handoffs remain open across activity changes, page refresh, park,
    resume, and server restart until explicitly answered, resolved, dismissed, superseded, tab-closed,
    browser-restarted/stopped, or task-ended.
  - Answering (`hook_prompt_submit` typed prompt, `task_message`, `answer_needs_you`), dismissing
    (`dismiss_task_question`), resolving/replacing (`tool_needs_you`), closing/ending the task,
    closing the tab, or clearing the browser action via the viewer API clears BOTH the persisted
    request and `BrowserSession::clear_action` / `clear_task_actions`, emits
    `UiEvent::BrowserActionCleared`, and publishes `task.updated`.
  - Legacy in-memory tab handoffs (present only on `BrowserSession::tabs()`) resolve their actual
    redacted `reason`, `tab_id`, and browser link instead of the generic fallback string.
- **NYM-003 (Specific system-generated stops):**
  - Open plan steps name the remaining open count and first outstanding step title (e.g.
    `in_progress`, `blocked`, or `pending`) plus a concrete next action (Reply/Resume or mark steps
    done).
  - Failing PR checks name the PR number, failed job(s), and failing test name(s) from Yulius CI
    (when available), link to the CI run or task, and provide a concrete next action.
  - Unfinished spec, unfinished goal, blocked chains, held prompts (without leaking typed prompt
    excerpts to non-person callers), `attention` flags, and parked Needs You states each produce
    specific `happened`, `action`, and `link` values.
  - Legacy `needs_you` (`why` alias), legacy `attention` (without reason), and legacy browser
    handoffs all produce actionable notices.
- **NYM-004 (Agent-recoverable failures & progressing checks):**
  - When a task has a newer Yulius CI run queued or running for its open PR, or a live unpaused
    non-playground task has an untold failed/errored Yulius CI run waiting for `pr_watch` delivery
    with no pending human request, `task_attention_facts` reflects Working / waiting-on-Yulius rather
    than Needs You.
- **NYM-005 (Timestamped task history with per-episode deduplication):**
  - Migration `0080_task_wait_messages.sql` adds `task_wait_messages` with
    `UNIQUE (task_id, episode)`.
  - Entering Needs You records one timestamped message (`source`, `happened`, `problem`, `action`,
    `link`, `decision`, `created_at`). Repeated polls (`tasks_list`, `task_get`, HTTP list/detail)
    for the same wait episode keep that single message and original timestamp.
  - Leaving Needs You or entering a new distinct wait episode marks open history entries resolved
    and records the new episode as a new timestamped message.
  - `TaskDetail` and `task_get` expose `wait_history` (bounded, newest first or chronological) with
    existing permissions and secret redaction.
- **NYM-006 (Consistent projections & notifications):**
  - `TaskSummary` carries `needs_you_notice` (and effective `needs_you` for browser handoffs).
  - `tasks_list` and `task_get` return the same explanation (`needs_you_notice`, `wait_reason`,
    and structured `needs_you` / `attention`) for the same permitted owner.
  - Web UI notifications (`notifications.js`) and chat notifications (`notify.rs` /
    `yulius_domain::notify`) include the specific explanation, next action, and target route/link.
- **NYM-007 (Web UI visibility & reachability):**
  - Task page (`taskview.js`) displays the actionable Needs You summary beside the status in the
    step strip AND the full message (what happened, what to do, where/link, timestamp, and actions
    including Open browser tab / Reply / Resume) in the attention bar, even when the agent stopped,
    parked, or has failing PR checks.
  - Sidebar Needs You list (`layout.js`), Tasks page (`pages/tasks.js`), and Task overview
    (`pages/taskinfo.js`) surface the actionable explanation and link to the task; Task overview
    and the task page History view display the timestamped Needs You history.
  - Browser panel (`browser.js`) lets a writer mark/clear a tab's requested action and updates
    live on `browser.action_cleared`.

## Plan

- [x] Reproduce the defect and specify unified Needs You reasons and handoff lifecycle
- [x] Persist browser handoffs and clear them when answered, superseded, closed, or ended
- [x] Compute specific Needs You explanations, progressing-check states, and deduplicated wait history
- [x] Expose consistent Needs You explanations and history across HTTP APIs, remote MCP, and notifications
- [x] Show actionable Needs You messages, history, and browser handoff controls in the web UI
- [x] Cover real browser handoffs, agent-recoverable failures, legacy records, and UI flows with regressions
- [x] Complete independent review, fix findings test-first, and update documentation
- [x] Verify green Yulius CI and deliver to dev

## Acceptance criteria

- **AC-1 (NYM-001, NYM-002, NYM-006, NYM-007):** A real `browser_request_user_action` handoff is
  visible to its permitted owner with its redacted reason and browser tab link on the task page
  (beside the status and in the attention bar), in the Needs You list, in notifications, and
  identically in `tasks_list` and `task_get` — even after activity leaves `needs_attention`, page
  refresh, park, resume, or server restart. Answering, dismissing, resolving, superseding, closing
  the tab, or ending the task clears the handoff in both SQLite and the running browser, and it
  stays cleared after refresh.
- **AC-2 (NYM-001, NYM-003, NYM-005, NYM-006, NYM-007):** Stopping with open plan steps (the #540
  shape: 5 open steps) immediately produces a visible explanation naming the open step count,
  first open step title, and concrete next action across the task page, Needs You list,
  notifications, `tasks_list`, `task_get`, and task history.
- **AC-3 (NYM-003, NYM-004, NYM-005, NYM-006, NYM-007):** A single failed test on an open PR (the
  #553 shape) reflects Working/waiting while an agent-recoverable CI failure is queued for delivery
  to the live agent or a newer CI run is progressing; when the agent stops without fixing it (or is
  parked), Needs You names the PR, failed job and failing test, link, and concrete next action.
- **AC-4 (NYM-005):** Repeated polls of `tasks_list`, `task_get`, and HTTP task list/detail during
  one wait episode keep a single timestamped message in `task_wait_messages`; a later distinct
  request records a new timestamped entry and resolves the prior one.
- **AC-5 (NYM-002, NYM-003, NYM-006):** Legacy in-memory browser handoffs (no `tasks.needs_you`
  row), legacy `attention` flags, and legacy `needs_you` (`why`) rows all render actionable
  explanations; secrets are redacted and existing view/work/owner access boundaries are enforced.

## Test plan

- **Unit & store tests:** `yulius-domain` (`notify.rs`), `yulius-store` (`task_wait_messages`
  uniqueness, episode deduplication, resolution, ordering), `yulius-app` (`task_questions.rs`,
  `pr_watch.rs`, `needs_you.rs`).
- **Integration tests (`crates/yulius/tests/`):**
  - Real browser handoff (`browser.rs` + `agent_hooks.rs`): visibility after activity change,
    refresh/re-read, park/resume, restart; clearing on answer/dismiss/resolve/supersede/tab-close/
    task-end; legacy in-memory-only browser handoff; secret redaction and permission boundaries.
  - System-generated stop with open steps (#540 shape) and failing PR check with failed job/test
    (#553 shape): HTTP `GET /api/sandboxes/{sid}/tasks`, `GET /api/tasks/{tid}`, remote MCP
    `tasks_list` and `task_get` parity, notification text, and `wait_history` deduplication across
    repeated polls and distinct subsequent requests.
  - Agent-recoverable CI failure and progressing newer CI run staying Working/waiting until
    human action is actually required.
- **Web E2E tests (`e2e/`):** Task page beside-status display and attention bar for stopped/parked
  open-step tasks, failing PR checks, browser handoffs (including clear & refresh), Needs You list
  reachability, task history rendering, and notification center detail.
- **Gates:** `scripts/dev quick`, scoped Rust nextest & Clippy, `cd e2e && npx e2e run`,
  `scripts/dev security` (report disabled if off), and green Yulius CI.

## Decisions

- **Persist browser handoffs in `tasks.needs_you` (with optional `browser_tab`) plus live browser
  sync:** Reuses the existing request persistence, spans trigger, permissions (`named`, owner/writer
  checks), dismissal/answer paths, and UI contracts instead of inventing a parallel request table,
  while keeping legacy in-memory `tab.action_requested` fallback so pre-upgrade handoffs work
  seamlessly. Recorded in [ADR-0414](../../docs/decisions/0414-actionable-needs-you-notices-and-wait-history.md).
- **Dedicated `task_wait_messages` table (`0080`) with `UNIQUE (task_id, episode)`:** Avoids
  conflicting with `SPEC-028` (`MSR-002`) assertions on the legacy `messages` table while providing
  atomic, poll-safe per-episode deduplication and durable timestamped history across restarts.
- **Server-computed `NeedsYouNotice` on `TaskSummary`, `TaskDetail`, `tasks_list`, `task_get`, and
  notifications:** Eliminates drift between web UI, remote MCP, and chat notifications; the web UI
  falls back gracefully for older servers without `needs_you_notice`.

## Evidence

- Baseline reproduction: `browser::browser_handoff_stays_visible_after_activity_change_and_clears_when_resolved` failed on unmodified `dev` with `"wait_reason": "A request for a person was pending when the task list was read."` and `needs_you: null` after `NeedsAttention -> Done -> Waiting`.
- Unit & store suites: `cargo test -p yulius-domain -p yulius-store -p yulius-app --lib` (675 `yulius-app` tests, 104 `yulius-store` tests including `wait_messages_deduplicate_per_episode_and_record_distinct_waits` and pinned schema v79 migration `0080_task_wait_messages.sql`, and all `yulius-domain` tests passed; re-verified `the_latest_schema_versions_keep_their_migrations` after merging `origin/dev` `5addbfbb8`).
- Integration suites:
  - `cargo test -p yulius --test main -- needs_you_messages:: browser:: agent_hooks:: telegram:: mcp_coordination::` (all passed after updating the legacy PR #1462 `wait_reason` expectation to the specific PR-named notice).
  - Post-Round-1 & post-merge (`136eac023796b6cc506a1174ab4434fd5ff86f4f`): `cargo test -p yulius --test main -- needs_you_messages:: browser_handoff browser_restart telegram_needs_you_notification` — 13/13 passed, covering real browser handoffs (inline `answer_needs_you` while `activity == NeedsAttention`, terminal prompt block, 0-viewer tab close, 2-viewer single `BrowserActionCleared` emission, `restart_browser` clearing, parked-task deletion clearing, orderly shutdown preservation, legacy #334 `tasks.attention` fallback across `NeedsAttention -> Done -> Waiting`, superseding tabs, park survival, viewer `clear_action`), 5 open steps (#540 shape) with deduplicated wait history, failing PR check (#553 shape) including superseded failed runs, re-entering resolved CI failure episodes, parked merged-PR non-phantom history, progressing CI `yulius_wait` web parity, held-prompt excerpt access boundaries, and vault + generic credential redaction across HTTP, MCP, wait history, and Telegram notifications.
- Linting & repository contracts: `cargo clippy -p yulius-app -p yulius-server -p yulius --all-targets -- -D warnings` (0 warnings), `python3 scripts/decision_slots.py`, and `scripts/dev quick` all passed.
- Web E2E suite: `cd e2e && npm ci --ignore-scripts && npx e2e run` — 97 passed, 1 skipped (live-agent test), 1184/1184 assertions passed; post-Round-1 re-run of `tests/needs-you-messages.e2e.ts` (4/4), `tests/attention.e2e.ts`, `tests/parked.e2e.ts`, and `tests/notifications.e2e.ts` passed with zero axe accessibility violations.
- Independent review (`reviewer-r1`, `gemini-4-neon-high`, high effort): Round 1 (`fa7b855b5382e44826ae1c32e6cfe52661696306`) returned 6 Major + 3 Minor findings; all 9 fixed test-first; Round 2 (`136eac023796b6cc506a1174ab4434fd5ff86f4f`, PR #1543) returned `approve` with one docs-only schema-version nit (fixed here, 0 product lines changed).
- Security scan & sandbox limits: `scripts/dev security` exited 0 reporting disabled (`Vidoc is turned off in Yulius, for this project or the whole instance; nothing was scanned`). `cargo-mutants` and `.tools/bin/macos-cc` are not installed in this sandbox; full gate + macOS cross-check run in Yulius CI on PR #1543.
