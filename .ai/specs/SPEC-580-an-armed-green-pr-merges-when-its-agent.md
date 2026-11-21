---
yulius-task: d3ba4ccb-fbf0-413f-862f-371da1a61fed
status: done
---

# SPEC-580: An armed green PR merges when its agent parks with a background `yulius wait`

## Goal

When an Antigravity (or Claude Code / Codex) agent arms `delivery_merge` on a green pull request, starts `yulius wait` in the background, and stops at its prompt, Yulius must recognize that the turn ended, show the task as waiting on Yulius rather than working, enter the PR into the merge queue promptly, report and act on merge conflicts instead of waiting, and never let an armed green PR wait silently past a bounded time without checking in and then alerting its person via Needs You.

## Context

Two independent live incidents on the Quartio instance (build `3fc0e5976`, 2026-11-21) exposed four linked root causes:

1. **Quartio-Studio/quartio-v2 task #338, PR #1498:**
   - Antigravity harness (`gemini-4-neon`), armed at head `74dfc55c9`, own CI run `9384f2cc` passed at 08:02 UTC, GitHub reported `MERGEABLE` / `CLEAN`.
   - The agent started `yulius wait` in the background and stopped at its prompt (terminal showed `[...] yulius wait running` at an idle prompt).
   - Yulius kept showing `shows=working, activity=working`, `delivery_status` kept saying `"Yulius tests it on top of the current integration branch as soon as your turn ends with everything committed"`, and no merge-train run started for 2.5 hours.
   - At 10:26 UTC a person's message caused the agent to take one more turn (ending with `fullyIdle: true`); within ~4 minutes the PR entered merge train #1498 (run `86a41cbc`). Nothing else changed.

2. **Quartio-Studio/yulius task #563, PR #1568:**
   - Antigravity harness (`gemini-4-neon`), armed at head `5fcf607c0`, own CI run passed at 07:37 UTC.
   - Same idle prompt with `[07:37:54] yulius wait running`, stalled for 3 hours with `shows=working, activity=working` and the same `"as soon as your turn ends"` note.
   - Additionally, `dev` moved and conflicted with PR #1568: GitHub reported `mergeable=CONFLICTING` / `mergeStateStatus=DIRTY`, while `task_delivery` reported `pr.conflicts=false` and kept the merge armed without telling the agent to resolve the conflict. After a person's message, the agent merged `dev` and re-armed at `b5e6db99d`.

Codex and Claude Code tasks on the same instance merged normally during the same hours.

### Verified Root Causes in Code

1. **Antigravity `Stop` with `fullyIdle: false` skipped `clean_stop` (`crates/yulius-app/src/hooks.rs`):**
   - `SPEC-528` introduced `let background = antigravity && !failed && input.fully_idle == Some(false);` and branched to `self.antigravity_work(&task.id, sent)` instead of `self.clean_stop(task)`.
   - Antigravity CLI sends `Stop` with `fullyIdle: false` whenever *any* background command is alive — including `yulius wait`!
   - Skipping `clean_stop` left `task.activity = Some(Activity::Working)` forever while `yulius wait` (or any background command) ran, never probed idle shells/waits (`probe_idle_shells`), and caused every merge-queue readiness check (`decidable_in`, `train_ready`, `train_member`, `keep_armed_current`, `candidate_wait_note`) to fail `task.activity.is_some_and(Activity::is_idle)`.
   - Additionally, `settle_clean_turn` (`crates/yulius-app/src/finish.rs`) never called `runtime.end_turn()` or updated `runtime.turn_closed_ms`. Because Antigravity transcripts emit no `TurnEnded`/`TurnComplete` record (unlike Claude Code's `turn_duration` and Codex's `task_complete`), `runtime.turn_in_progress()` remained `true` forever even after `fullyIdle: true`, blocking `pr_watch` check-ins and stalls (`!facts.working`, `facts.idle`).
   - Across the other harnesses: Claude Code and Codex both call `clean_stop` on every clean `Stop` and count `yulius wait` separately via `yulius-ptyhost::shells::count_background`, so they did not hit the `fullyIdle: false` bug; however, ending `turn_in_progress` in `settle_clean_turn` closes the window before transcript turn-end records arrive for all three harnesses, and PTY-based background command detection in `shells.rs` lets both Antigravity and Codex distinguish non-wait background commands from `yulius wait`.

2. **`delivery_view` hardcoded `pr.conflicts = false` and `pull_conflicts` could read a stale local `origin/<integration>` (`crates/yulius-app/src/delivery.rs`):**
   - `dlv_delivery_of` constructs `pr` via `pull_view(p, checks)`, which hardcodes `conflicts: false` ("Decided per task by `App::pull_conflicts`"), and `delivery_view` never called `self.pull_conflicts(task, &sandbox, pull)`. Consequently `task_delivery`, `delivery_status`, `TaskDetail.delivery`, and `merge_armed_task` ALWAYS saw `pr.conflicts == false`.
   - Furthermore, `pull_conflicts` trusted `local_conflict` even when `local_conflict` checked against a stale `origin/<integration>` while GitHub already reported `pull.mergeable == Some(false)`.
   - Finally, `merge_armed_task` (`crates/yulius-app/src/agent_wait.rs`) never checked `pr.conflicts` before keeping an armed green PR waiting behind `behind_integration` / `candidate_wait_note` (`"as soon as your turn ends"`), so a conflicting armed PR whose agent wasn't idle stayed armed silently instead of dropping the arm and telling the agent to resolve the conflict.

3. **No bounded check-in → Needs You escalation for stalled armed green PRs (`crates/yulius-app/src/pr_watch.rs`):**
   - `pr_watch::decide` only fires `STALL_AFTER_MS` (60 min) when `!facts.working` and sets `pr_stalled` (a label suffix, NOT `TaskGroup::NeedsYou`). When an armed green PR stalled (including with `working == true`), neither a timely check-in nor a Needs You notice ever fired.

### Non-goals

- Do not weaken merge-queue safety rules (exact-head testing, arm ordering, merge-driver union resolution from SPEC-491 M1, or worktree safety when a non-wait background command is running).
- Do not change the Antigravity CLI binary itself.

## Requirements

- **AWM-001 (Antigravity `Stop` settles the turn and distinguishes `yulius wait` from real background work):**
  - Every clean Antigravity `Stop` (`terminationReason != ERROR` and `!= TERMINAL_CUSTOM_HOOK`), whether `fullyIdle` is `true`, `false`, or omitted, ends the model turn (`clean_stop` → `Activity::Waiting` or `Activity::Done`, `runtime.end_turn()`, `turn_closed_ms` updated).
  - When `fullyIdle: false` is reported, Yulius inspects the live run's background processes (`background_probe`) and active `POST /internal/agent/wait` long-polls (`active_waits`):
    - If `yulius wait` is active and no non-wait background shell/command is running, `background_agents = 0`, `background_shells = 0`, `yulius_wait = true`: the task is idle for merge-queue readiness (`decidable_in`, `train_ready`, `train_member`, `keep_armed_current`, `candidate_wait_note`) and merges normally.
    - If real non-wait background work is running (either a non-wait background shell/PTY command detected by `shells.rs`, or `fullyIdle: false` with no `yulius wait` active, such as an in-process subagent), `background_agents = 1` / `background_shells >= 1`: the task stays `activity = Waiting` with `background_shells > 0` (preserving `SPEC-528` so it does not falsely enter Needs You while real background work runs, and keeping the worktree safe until that work ends). A subsequent autonomous `PreInvocation` or `Stop` with `fullyIdle: true` clears `background_agents`.
  - `settle_clean_turn` calls `runtime.end_turn()` and updates `runtime.turn_closed_ms` for all harnesses (Claude Code, Codex, Antigravity).
  - Process probe (`yulius-ptyhost::shells`) counts agent-owned child commands running on their own PTY (`tty_nr != 0 && tty_nr != agent.tty_nr`, excluding `yulius hook` and `yulius mcp`) as non-wait background shells unless they only run `yulius wait` (counted in `waits`).

- **AWM-002 (Task list never shows active "Working" for an idle agent parked on `yulius wait`):**
  - After a clean Antigravity `Stop` while parked with a background `yulius wait`, `task.activity` is `Waiting` (never `Working`), `background_shells` is `Some(0)`, and `yulius_wait` is `true`, so HTTP/MCP task summaries report `activity: "waiting"` and the web UI (`status.js`) renders `"Waiting for checks or merge"` (`tone: "queued"`), not `"Working"`.

- **AWM-003 (Accurate conflict detection in `delivery_view` and immediate conflict wake for armed PRs):**
  - `delivery_view` (and thus `task_delivery`, `delivery_status`, `TaskDetail.delivery`, and `merge_armed_task`) populates `pr.conflicts` using `App::pull_conflicts`.
  - When `pull.mergeable == Some(false)` on GitHub and `local_conflict` initially returns `Some(false)`, `pull_conflicts` refreshes the sandbox clone (`dlv_refresh_clone`) and re-evaluates `local_conflict` with the repository's merge drivers (preserving SPEC-491 M1 union merges when the refreshed base merges cleanly, and reporting `true` when the refreshed base conflicts or cannot be refreshed).
  - In `merge_armed_task`, when an armed green PR (`Hold::Free`) has `pr.conflicts == true`, Yulius drops the armed merge with actionable conflict-resolution instructions (naming conflicting files via non-destructive `preview_merge` when available) rather than keeping it armed with `"as soon as your turn ends"`. `decidable_in` treats a cached local conflict on an armed green `Hold::Free` PR as decidable so the 20 s pass handles it promptly.

- **AWM-004 (Bounded check-in → Needs You escalation for stalled armed green PRs):**
  - Nonterminal state: an armed green open PR (`Hold::Free`, not waiting on its own CI, no merge train running in the sandbox) that has not been tested on top of the integration branch or merged. Owner: `pr_watch`.
  - Exit triggers: PR merges, enters a merge train, moves to a new head, is disarmed, or its own checks cease to be green.
  - Failure/timeout contract (configurable in `WatchBounds` for deterministic tests):
    1. After `ARMED_CHECK_IN_AFTER_MS` (default: 10 minutes) with no progress (even if `working == true`), `pr_watch` sends one actionable check-in message into the agent's session naming the PR, head, elapsed time, and current reason it has not entered a candidate or merged.
    2. After `ARMED_NEEDS_YOU_AFTER_MS` (default: 10 minutes after the check-in, 20 minutes total) still on the same armed green PR with no progress, Yulius opens a persisted `NeedsYouRecord` (task #562 / `SPEC-578` mechanism) naming the PR, head, elapsed wait, problem, concrete next action, and task link — moving the task into `TaskGroup::NeedsYou`, recording one deduplicated timestamped wait episode in `task_wait_messages`, and surfacing consistently across HTTP, remote MCP, web UI, and notifications.
    3. If the PR subsequently makes progress (enters a train, merges, moves head, or disarms), Yulius automatically ends its own armed-stall `NeedsYouRecord` (by exact timestamp via `end_task_needs_you_at`, never clearing a newer person-directed request) and resolves its wait episode.

## Plan

- [x] Specify the four root causes, bounds, and acceptance criteria in SPEC-580
- [x] Write failing regression tests reproducing each fault before the fix
- [x] Settle Antigravity turn ends with background yulius wait and detect PTY commands in shells probe
- [x] Compute fresh PR conflict status in delivery views and drop conflicting armed merges
- [x] Add armed-green PR stall check-in and Needs You escalation with auto-resolution
- [x] Update ADR-0419, architecture docs, and recent changes
- [x] Complete independent review and address all findings test-first
- [x] Verify green Yulius CI and deliver to dev

## Acceptance criteria

- **AC-1 (AWM-001, AWM-002):** Using the real Antigravity hook adapter (`yulius hook antigravity Stop` with `fullyIdle: false`) while a background `yulius wait` is active, an armed green PR behind moved `origin/dev` immediately shows `activity = "waiting"`, `yulius_wait = true`, `background_shells = Some(0)` (never `activity = "working"`), passes `decidable_in` / `train_ready`, and enters a merge train and merges into `dev`.
- **AC-2 (AWM-001):** Using the real Antigravity hook adapter (`Stop` with `fullyIdle: false`) when a non-wait background command or subagent is active (and no `yulius wait`, or both `yulius wait` and a non-wait PTY command), `activity` becomes `"waiting"`, `background_shells >= 1`, `shows = "working"` (never falsely `needs_you`, preserving SPEC-528), and the merge queue waits until the non-wait command finishes; a subsequent `PreInvocation` / `Stop { fullyIdle: true }` clears it. Claude Code and Codex harnesses are verified not to suffer the same fault.
- **AC-3 (AWM-003):** When `origin/dev` advances on the remote with a real conflict against an armed green PR (while the local sandbox clone has not yet fetched `origin/dev` and GitHub reports `mergeable = false`), `task_delivery` reports `pr.conflicts = true`, and `merge_armed` drops the arm with actionable conflict instructions naming the conflicting files rather than keeping it armed with `"as soon as your turn ends"`. A union-driver collision (`CHANGES.md`) still reports `pr.conflicts = false` after clone refresh (SPEC-491 M1).
- **AC-4 (AWM-004):** An armed green PR that cannot enter a train or merge (e.g., dirty worktree or non-wait background shell or stuck working turn) receives a check-in after `armed_check_in_after_ms`; if still unmoved after `armed_needs_you_after_ms`, it enters `TaskGroup::NeedsYou` with a `NeedsYouNotice` (`tasks_list`, `task_get`, HTTP summary/detail, and `wait_history` parity via SPEC-578). When it later merges or enters a train, that Yulius-opened Needs You automatically clears.

## Test plan

- **Unit tests (`yulius-ptyhost::shells`):** PTY-hosted child commands (`cargo test` vs `yulius wait` vs `bash -c 'yulius wait'` vs `yulius mcp` / `yulius hook`), both alone and combined.
- **Unit tests (`yulius-app::pr_watch`):** `decide` state machine for armed green PR check-in at `armed_check_in_after_ms`, escalation signal at `armed_needs_you_after_ms`, suppression while another train runs or CI is active, reset and auto-clear on progress/merge/head move/disarm.
- **Integration tests (`crates/yulius/tests/`):**
  - Real `antigravity_hooks` adapter payload (`Stop { fullyIdle: false }`) + active `yulius wait` + armed green PR behind moved `dev` → `activity == Waiting`, `yulius_wait == true`, `background_shells == Some(0)`, PR joins merge train and merges; also verify Codex & Claude Code parity.
  - Antigravity `Stop { fullyIdle: false }` with non-wait background work (no `yulius wait`, and `yulius wait` + non-wait command) → `activity == Waiting`, `background_shells >= 1`, `shows == working`, SPEC-528 preserved.
  - Stale local `origin/dev` + GitHub `mergeable = false` → `task_delivery` reports `pr.conflicts = true`, `merge_armed` drops arm with conflict instructions naming conflicting files; union-driver test still reports `conflicts = false`.
  - Armed green PR stall bound with controlled clock → check-in typed after `armed_check_in_after_ms`, Needs You raised with SPEC-578 `NeedsYouNotice` + `wait_history` after `armed_needs_you_after_ms`, auto-cleared once PR moves/merges.
- **Gates:** Scoped `cargo nextest` / `cargo test`, `cargo clippy --all-targets -- -D warnings`, `scripts/dev quick`, `python3 scripts/decision_slots.py`, `scripts/dev security` (report disabled if off), and green Yulius CI.

## Decisions

- **Settle every clean Antigravity `Stop` as `Activity::Waiting` and track non-wait background work via `background_agents`/`shells.rs` instead of keeping `Activity::Working`:** Matches Claude Code's canonical lifecycle model (turn ended = `Waiting`, background work = `background_shells > 0`, `yulius wait` = `background_waits > 0` / `active_waits > 0`), fixes both merge-queue readiness and task-list display at the root cause, and preserves SPEC-528 without breaking `turn_in_progress`. Recorded in [ADR-0419](../../docs/decisions/0419-antigravity-stop-settles-turn-and-armed-green-prs-have-a-stall-bound.md).
- **Refresh the sandbox clone on `pull.mergeable == Some(false)` disagreement in `pull_conflicts` and populate `pr.conflicts` in `delivery_view`:** Eliminates stale-base false negatives while preserving SPEC-491 M1 union merge drivers.
- **Reuse SPEC-578 (`tasks.needs_you` + `NeedsYouNotice` + `task_wait_messages`) for armed-green PR stall escalation (10 min check-in, +10 min Needs You):** Delivers consistent actionable notices, history, and notifications across every surface without inventing a parallel alert path, with exact-timestamp auto-clear (`end_task_needs_you_at`) on progress.

## Evidence

### Baseline reproducing failures (before production changes)
Ran `cargo test -p yulius --test main -- antigravity_armed_merge:: --nocapture` on unmodified `dev` (0 passed, 4 failed):
- `antigravity_armed_green_pr_with_yulius_wait_merges_and_is_not_working`: failed (`left: Some(Working), right: Some(Waiting)` — Antigravity `Stop { fullyIdle: false }` skipped `clean_stop` while `yulius wait` ran).
- `antigravity_stop_with_non_wait_background_work_preserves_spec_528_and_waits_for_completion`: failed (`left: Some(Working), right: Some(Waiting)`).
- `armed_green_pr_conflict_reports_conflicts_true_and_drops_arm`: failed (`task_delivery` returned `pr.conflicts: false` on a conflicting PR with stale local `origin/dev`).
- `stalled_armed_green_pr_checks_in_then_escalates_to_needs_you_and_auto_clears`: failed (`expected SPEC-578 needs_you_notice after armed-PR stall bound`).

### Post-fix verification (on approved HEAD `89e39e7829b7c03da3a6dd9eaaa26db9471e1d95`)
- `cargo test -p yulius-ptyhost shells::` — **17 passed, 0 failed** (including `pty_hosted_commands_distinguish_yulius_wait_from_real_background_work`).
- `cargo test -p yulius-app --lib agent_wait::tests::` — **9 passed, 0 failed** (including `active_wait_guard_balances_and_never_underflows`).
- `cargo test -p yulius-app --lib pr_watch::tests::` — **20 passed, 0 failed** (including `armed_green_stall_checks_in_then_escalates_and_resets_on_progress` and `untold_resets_armed_check_in_on_new_head_and_preserves_on_same_head`).
- `cargo test -p yulius --test main -- delivery::overlapping_parked_status_reads_bound_git_workers_after_timeouts --exact` — **1 passed, 0 failed** (3.38 s; verifies parked-task `delivery_view` conflict checks respect `PARKED_READS` and `status_deadline()`).
- `cargo test -p yulius --test main -- antigravity_armed_merge::` — **10 passed, 0 failed**:
  - `antigravity_armed_green_pr_with_yulius_wait_merges_and_is_not_working`
  - `antigravity_stop_with_non_wait_background_work_preserves_spec_528_and_waits_for_completion`
  - `antigravity_stop_with_both_shell_and_wait_holds_until_shell_exits`
  - `antigravity_stop_with_active_agent_wait_rpc_settles_without_probe_waits`
  - `claude_and_codex_armed_green_pr_with_yulius_wait_parity`
  - `armed_green_pr_conflict_reports_conflicts_true_and_drops_arm`
  - `union_driver_collision_with_stale_clone_reports_conflicts_false`
  - `stalled_armed_green_pr_checks_in_then_escalates_to_needs_you_and_auto_clears`
  - `stalled_armed_green_pr_delivers_live_pty_check_in_then_escalates`
  - `stalled_armed_green_pr_already_merged_on_github_does_not_escalate`
- `cargo test -p yulius --test main -- agent_hooks::antigravity_` — **3 passed, 0 failed** (all existing SPEC-528 tests preserved).
- `cargo test -p yulius --test main -- merge_train::github_s_flag` — **1 passed, 0 failed** (SPEC-491 M1 preserved).
- `cargo test -p yulius --test main -- parking::armed_notice::` — **16 passed, 0 failed** (origin/dev armed-notice admission guards preserved).
- `cargo clippy -p yulius-ptyhost -p yulius-app -p yulius --all-targets -- -D warnings` — **0 warnings**.
- `cargo clippy -p yulius-ptyhost --all-targets --target aarch64-apple-darwin -- -D warnings` — **0 warnings** (verified by independent reviewer).
- `python3 scripts/decision_slots.py` and `scripts/dev quick` — **PASS** (16 crates, 7 skills, local Markdown links, no conflict markers, unique decision numbers, `cargo fmt --all -- --check`).
- `scripts/dev security` — **disabled** (Vidoc is switched off in Yulius for this project/instance; nothing was scanned).

### Independent review
- Independent review by fresh `reviewer` agent (`gemini-4-neon-high`, high effort):
  - Round 1 on `b1a055e47`: REQUEST CHANGES (1 blocking, 4 major, 1 minor, 1 nit — all fixed test-first in `8fab62b43`).
  - Round 2 on `8fab62b43c99ca877c6a4f0a7230f7edeefa70fd`: **APPROVE** (all 7 findings verified fixed with dedicated regression tests; 2-line duplicate doc comment nit cleaned up in `finish.rs`).
- Independent review in a fresh session by `reviewer-ci` (`gemini-4-neon-high`, high effort) after bounding parked-task conflict checks in `delivery_view` (`89e39e7829b7c03da3a6dd9eaaa26db9471e1d95`): **APPROVE** (0 blocking/major/minor findings; 1 optional spec-evidence nit applied here).

### Remaining validation limits
- Full workspace `scripts/dev macos` and full CI gate (`scripts/dev ci`, full integration suite, and `scripts/dev mutants`) are deferred to Yulius CI on the pull request head per `AGENTS.md`.
