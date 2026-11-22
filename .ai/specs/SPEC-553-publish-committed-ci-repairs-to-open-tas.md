---
yulius-task: 14dabe7e-01de-4b7b-b00e-3d8b5019b81d
status: done
---

# SPEC-553: Parked session terminal resume, automatic PR wake, and committed CI repair publication

## Goal

Fix the full incident observed on `quartio-v2` tasks #322, #323 and #324:

1. **Primary — Parked web terminal & supervisor wake:** A parked task's terminal must clearly distinguish normal idle parking from a crash or operator stop, always show the parked banner with a visible `Resume` action (and `Reply` when waiting for the owner), stop blinking an inert live cursor, and wake the exact saved session (rollout/conversation ID and worktree) through the supervisor when the owner sends ordinary input or clicks `Resume` — while strictly preserving operator stop, crash, and ownership/consent boundaries.
2. **Automatic wake on actionable check results while preserving safe parking during CI:** Idle sessions continue to park safely while CI runs; when an actionable PR/CI result arrives (failed run, errored run, green unarmed run, conflict, failed publication, or untold merge with remaining work), `pr_watch` automatically resumes the parked session in its exact saved rollout/worktree once (deduplicated across ticks and restarts), never waking on human-decision holds or operator-stopped/crashed/paused tasks.
3. **Concrete trigger — Eligible committed CI repairs on existing open task PRs:** When an existing task-owned open pull request is on a failing previous head and the worktree holds a clean committed fast-forward repair on the same feature branch, Yulius publishes it and triggers exact-head CI even while the final verification step remains open, and surfaces failed publication cleanly without duplicate spam.

## Context

- **Observed production incidents (Quartio VM, `quartio-v2`).**
  - Tasks #322 / PR #1453 (`3a3b0b620` vs PR head `6ba847be1`), #323 / PR #1448 (`c36778826` vs PR head `09fd377d3`) and #324 / PR #1444 (`4a2dfd495` vs PR head `44ca29558`) each committed a tested repair in a clean worktree while their final verification step remained open. All three PRs stayed on their previous failing heads; #324 parked.
  - Screenshot of #324: the task terminal ended with Codex's raw exit footer `To continue this session, run: codex resume 01a1f5b6-70ce-7c01-9ec5-0fbf8af01570` and left an inert terminal with a live blinking cursor and no parked banner or visible Resume action while work remained.
  - Second occurrence at 01:03 Warsaw: #323 showed `"Your turn · parked"`, step 5/5, and Codex's exit footer with resume ID `01a1f5b5-48a9-7361-af4b-710fec0b29fe` while its repaired PR already had CI running and no human decision was pending. An ordinary Yulius task message resumed the same rollout and worktree on the VM for both #323 and #324.
- **Root causes verified in code.**
  1. `isEndedStatus` (`crates/yulius-server/web/assets/termsocket.js:23`) omitted `'parked'`, so `terminal-core.js:238` cleared `exited = null` on every `state` frame (including page open/reload). The existing info-tone parked banner (`status.js:37`) and `Resume` button (`taskview.js:222`) never rendered, leaving only the saved PTY screen (Codex's exit footer) and a blinking cursor.
  2. `taskview.js` had no `onExitedInput` wake handler for parked terminals, `renderAttention` hid the waiting bar for `status === 'parked'`, `sendReply` called `typeAndSubmit` into the dead PTY for the owner, and `App::terminal_input` (`crates/yulius-app/src/terminal.rs:230`) returned `Conflict("the task is not running")` on parked tasks instead of waking them through `resume_parked`.
  3. `App::watch_prs` (`crates/yulius-app/src/pr_watch.rs:528`) only scanned `live_attempts()` (running tasks), so a task that safely parked while CI ran was never automatically resumed when CI finished with an actionable check result.
  4. `App::try_auto_open_pr` (`crates/yulius-app/src/delivery.rs`) blocked open-PR CI repairs behind `plan_hold(..., DeliveryAsk::TurnEnd)`; `dlv_observe` skipped `has_changes` for `DeliveryStage::PrOpen`; `try_auto_open_pr` did not trigger `ci_sync_sandbox` immediately after pushing; `agent_wait` did not publish eligible repairs on start/reconnect; and failed publications were omitted from `delivery_view` and `pr_watch`.
- **Explicit non-goals.**
  - Opening a new PR (or post-merge follow-up PR) with open steps at `TurnEnd` (SPEC-484 PLN-002 stays unchanged).
  - Generic Codex notification/reply-queue alerts (tracked separately in task #534).
  - Deploying or restarting the shared Quartio VM or mutating live `quartio-v2` PR heads.

## Requirements

SPEC-612 supersedes automatic parked-session wakes and finished-task repair instructions. A person resumes a parked task. Working project tasks own integration repairs.

- **PRK-101 (parked web terminal presentation and crash distinction).**
  - `isEndedStatus('parked')` in `termsocket.js` returns `true`, so opening, reloading, or receiving a `state` frame for a `parked` task keeps `exited = { status: 'parked', ... }` and always renders the calm info-tone parked banner (`status.js:37`) with a visible `Resume` action for the owner — clearly distinct from crashes (`stopped_crashed`, `warn`), external kills (`stopped_killed`, `warn`), and operator stops (`stopped_cancelled`, `"You stopped this task."`).
  - `terminal-core.js` stops cursor blinking while `exited` is set so an ended/parked screen never impersonates a live cursor.
  - `taskview.js` `renderAttention` also renders a calm parked waiting bar for `status === 'parked' && activity === 'waiting' && group === 'needs'`, offering `Resume` and `Reply` to the owner and a read-only parked notice without `Resume`/`Reply` to non-owners.
- **PRK-102 (ordinary owner input and Resume wake through the supervisor with continuity).**
  - Clicking `Resume` (`POST /api/tasks/{tid}/continue`) or submitting `Reply` / phone quick input on a parked task wakes it through `App::resume_parked` (optional `ContinueTask.prompt`), preserving the exact saved `provider_session_id` (Codex rollout ID `codex resume <id>`, Claude `--resume <id>`, Antigravity `--conversation <id>`) and `worktree_path`.
  - Typing ordinary input into a parked task's terminal wakes it through the supervisor: client-side `onExitedInput` (for `exit.status === 'parked'`, owner with write access, non-archived, ignoring bare `\x03`/`\x04`/`\x1b`) buffers typed characters, invokes `continueTask` once, shows `"Resuming parked session…"`, and replays buffered input once running; server-side `App::terminal_input` likewise wakes a `TaskStatus::Parked` task through `resume_parked` for authorized owner typing (`may_write`, `!scroll`, `is_typing`, non-interrupt).
  - Operator stop (`stopped_cancelled`), `closed`, crashed/killed/lost/failed states, archived tasks, and non-owners are NEVER woken by terminal input or `onExitedInput`.
- **PRK-103 (safe parking during CI and automatic wake on actionable PR/CI results).**
  - Idle tasks whose own work is complete can park while CI runs on the recorded PR head (`Queued`/`Running`), freeing session slots and memory. An unpublished clean fast-forward repair keeps its session until publication, including while a human request is open. [SPEC-558 parking eligibility](SPEC-558-parked-codex-tasks-no-codex-exit-screen.md).
  - `App::watch_prs` watches `TaskStatus::Parked` tasks (non-archived, not paused, not held by update, no `person_must_act`, open PR or untold merge). It never wakes on check-ins, stalls, or in-progress (`Queued`/`Running`) CI. When an actionable event arrives (failed run, errored run, unarmed green run, conflict, actionable failed publication, or untold merge while work remains / not `Activity::Done`), `watch_prs` resumes the parked task via `resume_parked(&task.id, Some(notice))` in its exact saved session and worktree, and persists the event key in `pr-notices.json` so duplicate ticks and restarts never double-wake it.
- **REP-001 (domain eligibility for open-PR CI repairs).** A pure domain rule (`yulius_domain::delivery::evaluate_open_pr_repair`) permits publishing a local commit with open plan steps only when the PR is an existing task-owned open PR on the task's non-protected feature branch targeting the integration branch (`!from_fork`), previous PR CI failed (code failure, not infra `error`), no merge is armed, no human decision is pending (`needs_you`, held prompt, browser request, `NeedsAttention`, pause, or `Blocked` step), worktree HEAD is on `task.branch`, worktree is confirmed clean (fail-closed), spec and required plan are valid, and local HEAD is a strict fast-forward descendant of `pull.head_sha`.
- **REP-002 (publish eligible repair and trigger exact-head CI).** At `TurnEnd` (`try_auto_open_pr`), `agent_wait` start/reconnect, and `watch_prs` recovery, eligible repairs are deduplicated via `claim_auto_head`, pushed plainly via `dlv_push_head` (never force-pushed), and immediately followed by `ci_sync_sandbox` when Yulius CI is enabled.
- **REP-003 (safeguards preserved).** New PRs, post-merge follow-ups, non-failing/armed open PRs, dirty worktrees (checked for both `None` and `PrOpen`), protected/fork/mismatched/wrong-branch/divergent branches, review/merge gates, ownership, and human decisions keep all existing safeguards.
- **REP-004 (actionable failed publication without duplicate spam).** Failed or blocked repair publication records an actionable `PushNote` shown in `yulius wait`, `auto_open_pr`'s terminal notice, `delivery_view` (`delivery_status`), and (once per `(head, note fingerprint)` persisted in `pr-notices.json`) `pr_watch` when `yulius wait` is not active.

## Plan

- [x] Specify open-PR CI repair publication, wait, parking, and notification rules (SPEC-553 and ADR)
- [x] Add domain open-PR CI repair eligibility rules with unit tests
- [x] Publish eligible committed CI repairs on open task PRs and trigger exact-head CI in delivery
- [x] Integrate yulius wait, pr_watch restart recovery, actionable failed publication, and parking protection
- [x] Fix parked web terminal presentation, owner input wake, and parked-task automatic PR/CI wake
- [x] Add Rust integration and web e2e regression tests for parked terminal resume, automatic PR wake, and CI repairs
- [x] Update architecture and changelog docs, verify checks, complete independent review, and merge into dev

## Acceptance criteria

- **AC-1 (parked web terminal banner, cursor, and Resume/Reply; PRK-101, PRK-102).** Opening or reloading a parked Codex/Claude task shows the info-tone parked banner with visible `Resume` (distinct from a crash or operator stop), disables live cursor blink, shows `Resume` and `Reply` in the waiting bar for the owner, and hides control buttons from non-owners.
- **AC-2 (ordinary owner input and supervisor continuity; PRK-102).** Typing ordinary input into a parked terminal (or submitting `Reply` / clicking `Resume`) wakes the parked task through the supervisor (`resume_parked`), preserving the exact `provider_session_id` (Codex rollout ID) and worktree; typing into an operator-stopped (`stopped_cancelled`), closed, crashed, or someone else's task never wakes it.
- **AC-3 (safe parking while CI runs + automatic wake on actionable check results; PRK-103).** A task with CI running on its open PR can still park safely after the grace period; once CI finishes with an actionable result (passed unarmed, failed, errored, conflict, or failed publication), `watch_prs` automatically resumes the parked session once with the exact rollout ID/worktree and does not double-wake across passes or restarts, and never wakes on `needs_you` or operator-stopped tasks.
- **AC-4 (open-PR CI repair circular wait fixed with safeguards; REP-001, REP-002, REP-003).** A clean committed fast-forward repair on a failing task-owned open PR publishes and queues exact-head CI at `TurnEnd` and `yulius wait` even while the final verification step is open; new PRs, non-failing/armed PRs, dirty worktrees, protected/fork/mismatched/wrong-branch/diverged branches, review/merge, and pending human decisions remain protected.
- **AC-5 (actionable failed publication without duplicate spam; REP-004).** Failed/blocked publication appears in `yulius wait`, `delivery_status`, and `pr_watch` (once per `(head, message)` across restarts, suppressed while `yulius wait` is active).

## Test plan

- **Unit tests (`yulius-domain`, `yulius-app`)**: `evaluate_open_pr_repair` matrix, `pr_watch::decide` / `news_due` deduplication and parked-event filtering, `ParkFacts` and `terminal_input` wake helpers.
- **Rust integration tests (`crates/yulius/tests/delivery.rs`, `pr_watch.rs`, `parking.rs`, `terminal_ws.rs`)**:
  - Circular wait reproduction & fix + exact-head CI trigger + all safeguards + `yulius wait` / restart recovery + failed publication in `yulius wait`, `delivery_status`, `pr_watch`.
  - Parked session wake via WebSocket `terminal_input` and `continue_task` with prompt, proving exact `provider_session_id` and worktree continuity, and proving `stopped_cancelled` / `stopped_crashed` / non-owner input is refused.
  - Safe parking while CI runs + `watch_prs` automatic wake of a parked session on actionable check results (failed and green unarmed), once across restarts, never while CI is still running or when `needs_you` is open.
- **Web UI e2e tests (`e2e/tests/parked.e2e.ts`)**:
  - `isEndedStatus('parked') === true` and parked Codex terminal renders the info-tone parked banner with visible `Resume` (distinct from `stopped_crashed` warn banner and `stopped_cancelled`), stops cursor blink, wakes via ordinary terminal typing or `Resume` or `Reply`, and never wakes `stopped_cancelled` or someone else's task.

## Decisions

- **Recorded in [ADR-0391](../../docs/decisions/0391-publish-committed-ci-repairs-to-open-task-prs.md)**:
  - Treat `parked` as an ended terminal session state in `termsocket.js` (`isEndedStatus`), stop cursor blinking, and wake parked sessions on owner terminal input / Reply / Resume through `resume_parked` while keeping operator stop, crash, and ownership boundaries intact.
  - Preserve safe idle parking while CI runs and extend `pr_watch` to automatically resume parked tasks once per actionable PR/CI event (persisted in `pr-notices.json`).
  - Publish eligible clean committed CI repairs on existing task-owned failing open PRs even while verification steps remain open, with fail-closed cleanliness/branch/fast-forward checks and immediate `ci_sync_sandbox`.

## Evidence

- **Quick repository contracts & formatting (`scripts/dev quick`)**: passed (`check_repository.py` + `cargo fmt --all -- --check`).
- **Lint (`cargo clippy -p yulius-contracts -p yulius-domain -p yulius-app -p yulius-server --all-targets -- -D warnings`)**: passed with 0 warnings.
- **Rust unit tests (`cargo test -p yulius-domain -p yulius-app -p yulius-server --lib`)**: passed (including `delivery::tests::open_pr_repair_eligibility_enforces_every_safeguard`, `pr_watch::tests::failed_publication_notice_is_told_once_and_counts_as_news_due_until_told`, `terminal::tests::parked_input_wake_distinguishes_typing_from_control_and_noise`).
- **Rust integration tests (`cargo test -p yulius --test main -- ...`)**: passed (113/113 across `delivery::`, `parking::`, `pr_watch::`, `terminal_ws::`; 25/25 `parking::` post-merge; all 7 SPEC-553 integration tests + `delivery::a_push_clears_only_the_note_about_its_own_head` passing after Round 2 review fixes):
  - `delivery::a_clean_committed_ci_repair_publishes_to_failing_open_pr_with_verification_open`
  - `delivery::open_pr_ci_repair_preserves_safeguards_and_surfaces_actionable_failures` (including detached HEAD at same SHA)
  - `delivery::open_pr_ci_repair_queues_exact_head_ci_and_recovers_via_pr_watch_without_stale_notice` (including Pass 2 silence)
  - `pr_watch::failed_publication_and_superseded_repair_notices_in_pr_watch` (real live PTY + `FakeGitHub`: one-time `"publish"` delivery, no stale `"publish"` after clean follow-up commit, Pass 1 & Pass 2 stale-failed-run suppression + negative control)
  - `parking::ordinary_owner_terminal_input_and_continue_prompt_wake_parked_session_with_continuity` (including oversized `ContinueTask.prompt` 422 rejection)
  - `parking::parked_task_parks_safely_during_ci_and_auto_wakes_on_actionable_check_results` (including no-usable-account preflight and restart `pr-notices.json` deduplication)
  - `parking::unpublished_open_pr_ci_repair_defers_parking_unless_needs_you_or_ci_running`
- **Web UI end-to-end suite (`cd e2e && npx e2e run`)**: full suite passed (94 passed, 1 skipped of 95 files; 1144 passed, 1 skipped of 1145 tests); after Round 1 review fixes, `npx e2e run tests/parked.e2e.ts` passed 5/5 (covering calm `.waiting` class, `data-cursor-blink="false"/"true"`, initial-load parked page, `stopped_crashed` & `stopped_cancelled` typing refusal, multi-character typing across `parked` -> `starting` -> `running` with zero `.term-notice` errors, and non-owner boundary) and `npx e2e run tests/terminal.e2e.ts tests/attention.e2e.ts` passed 42/42.
- **Mutation testing (`scripts/dev mutants`)**: not available in this sandbox (`cargo mutants --list failed: error: no such command: mutants`); reported honestly as unrun rather than passed. Every Round 1, Round 2, and Round 3 review finding was addressed with a targeted regression test.
- **Independent review (fresh read-only sessions, Rounds 1–4)**: Round 4 approved exact head `c9d39e3adb2aeb51d990af8263f9de15ef6a7369` with zero remaining findings after resolving all Round 1 (8), Round 2 (5), and Round 3 (2) findings with regression tests.
- **Security scan (`scripts/dev security`)**: disabled in Yulius for this project/instance (`Security scan disabled: Vidoc is turned off in Yulius, for this project or the whole instance; nothing was scanned`); reported as disabled, not passed.
- **Limits**: Live Quartio VM (`quartio-v2` #322/#323/#324) was not modified, restarted, or deployed from this task; live Codex rollout continuity on the VM was verified by the operator and covered locally through `provider_session_id` + `worktree_path` supervisor tests and Web UI mock e2e tests.
