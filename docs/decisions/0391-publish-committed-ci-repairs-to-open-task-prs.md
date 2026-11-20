# 0391. Parked terminal resume, automatic PR wake, and open-PR CI repair publication

## Status

Accepted for SPEC-553. Amends ADR-0325 (SPEC-484 PLN-002) for existing failing open pull requests and refines ADR-0367 (SPEC-512 PRK-002/004/005) for parked task terminals and automatic wake on actionable PR/CI events.

## Context

In production (`quartio-v2` #322, #323, #324), parked Codex tasks showed Codex's raw exit footer (`To continue this session, run: codex resume <id>`) in an inert web terminal with a blinking cursor and no parked banner or visible `Resume` action, because `isEndedStatus` omitted `'parked'` and `terminal_input` refused keystrokes on parked sessions. Meanwhile, tasks that safely parked while CI ran were never automatically woken by `pr_watch` when CI finished because `watch_prs` only scanned `live_attempts()`. Additionally, `try_auto_open_pr` blocked clean committed CI repairs on failing open PRs while verification steps remained open, creating a circular wait before parking.

## Decision

1. **Parked terminal presentation and owner-input wake.** `isEndedStatus('parked')` returns `true` so the terminal always renders the info-tone parked banner and `Resume` button (distinct from `stopped_crashed` warn banners and `stopped_cancelled`), stops blinking the cursor, and displays a parked waiting bar with `Resume`/`Reply` for the owner. Ordinary owner typing in a parked terminal (client `onExitedInput` with key buffering and server `terminal_input`) and `POST /api/tasks/{tid}/continue` (with optional `prompt`) wake the task via `App::resume_parked`, preserving its exact `provider_session_id` (rollout/session ID) and worktree while never waking operator-stopped, crashed, closed, archived, or non-owned tasks.
2. **Safe parking during CI with automatic wake on actionable PR/CI events.** Idle tasks continue to park safely while CI runs (`Queued`/`Running`). `App::watch_prs` also watches `TaskStatus::Parked` tasks (non-archived, unpaused, no `person_must_act`) and resumes them once via `resume_parked(&task.id, Some(notice))` on actionable events (red/errored run, unarmed green run, conflict, failed publication, or untold merge with remaining work), deduplicated across passes and restarts via `pr-notices.json`, never waking on check-ins, stalls, or active CI.
3. **Narrow open-PR CI repair publication.** A pure domain rule (`yulius_domain::delivery::evaluate_open_pr_repair`) publishes clean committed fast-forward CI repairs on existing task-owned failing open PRs at `TurnEnd`, `yulius wait`, and `pr_watch` recovery (deduplicated via `claim_auto_head`), triggers `ci_sync_sandbox` immediately when Yulius CI is enabled, checks worktree cleanliness for `DeliveryStage::PrOpen`, and surfaces failed publication once across `yulius wait`, `delivery_status`, and `pr_watch`.

## Alternatives

- **Block parking for all tasks with active CI.** Rejected: holds live session slots and memory during long CI runs when safe parking plus automatic wake on check completion frees capacity without stranding tasks.
- **Wake all ended states on terminal keystrokes.** Rejected: violates operator stop (`stopped_cancelled`) and crash investigation boundaries; only normal idle `parked` sessions wake on owner input.
- **Bypass `plan_hold` for all open-PR updates.** Rejected: violates SPEC-484 PLN-002 for pending/armed multi-slice PRs.

## Consequences

- Parked Codex, Claude Code, and Antigravity tasks clearly explain their parked state, distinguish it from crashes/stops, and resume seamlessly into the same rollout/session and worktree on `Resume`, `Reply`, ordinary owner input, or actionable CI results.
- Safe parking during CI is preserved, circular waits on failing open PRs are broken, and all ownership, consent, stop, worktree, branch, and merge safeguards remain intact.
