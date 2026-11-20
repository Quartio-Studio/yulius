# 0414. Actionable Needs You notices, persisted browser handoffs, and wait-episode history

## Status

Accepted for [SPEC-578](../../.ai/specs/SPEC-578-show-an-actionable-message-whenever-a-ta.md).

## Context

Tasks could enter the Needs You group without telling the person what happened, what to do, or where to act:
- `browser_request_user_action` only recorded an in-memory tab flag and `tasks.attention`; once activity moved from `NeedsAttention` to `Done` or `Waiting`, `App::attention_view` returned `None` while the tab stayed flagged, degrading remote reads to a generic placeholder and leaving stale handoffs attached.
- System-generated stops (unfinished plan steps, unfinished specs or goals, failing pull request checks, parked tasks) emitted no structured action or target link, and the web UI suppressed the attention bar on failing pull requests.
- Untold failed Yulius CI runs on a live session and stale failing PR observations superseded by a newer queued or running CI run could briefly show Needs You even though the agent or CI was already progressing.

## Decision

1. **Unified notice projection (`NeedsYouNoticeView`)**: `App::resolve_needs_you_notice` produces one redacted, single-line bounded explanation (`source`, `episode`, `happened`, optional `problem`, `action`, `link`, `decision`, optional `browser_tab`, `at`, `at_ms`) for every task in `TaskGroup::NeedsYou`. Both HTTP (`TaskSummary.needs_you_notice`) and remote MCP (`tasks_list` and `task_get`) project it identically for the same permitted caller, never including a held prompt's typed excerpt so task-agent callers stay within the QST-001 access boundary.
2. **Persisted browser handoffs & full clearing**: `browser_request_user_action` persists a `NeedsYouRecord` (linking `/s/{slug}/browser?tab={tab_id}`, deriving `browser_tab` from the link without altering `NeedsYouRecord` storage) and supersedes prior tab requests on the same task. Legacy records without `tasks.needs_you` fall back to the live browser tab request and stored attention timestamp/link. Answering, dismissing, resolving (`needs_you {resolved: true}`), superseding, closing the tab, sending viewer `clear_action`, or ending the task clears the tab request, persisted `needs_you`, and legacy browser `tasks.attention`, broadcasting `browser.action_cleared`.
3. **Agent-recoverable & progressing CI checks**: On a live unpaused non-playground session with no open human request, an untold failed or errored Yulius CI run keeps `pr_news_due` true (`Working`) until `watch_prs` delivers it, and a newer `Queued` or `Running` Yulius CI run upgrades a stale `PullSignal::Failing` observation to `PullSignal::InReview`. Once human action is needed, the notice names the PR, CI run id, failed job(s), failing test(s), and `/s/{slug}/ci?run={run_id}` link.
4. **Deduplicated wait-episode history**: Migration `0080_task_wait_messages.sql` adds `task_wait_messages` with `UNIQUE (task_id, episode)`. Repeated polls of the same wait episode keep one timestamped row; leaving Needs You or entering a distinct later episode resolves prior open rows and inserts a new row. Wait history is exposed in `TaskDetail.wait_history` and `task_get`, rendered in the task page and task overview, and used to include specific reasons and links in notifications.

## Alternatives

- Writing system wait messages into the existing `messages` table would conflict with `SPEC-028 MSR-002` (which requires `messages` to hold only inter-task messages) and would lack atomic per-episode deduplication across polls.
- Returning `tasks.attention` unconditionally regardless of `activity` would resurrect cleared terminal prompts whenever a task transitioned back to `waiting`.

## Consequences

- Every task entering Needs You immediately shows what happened, what to do, and where to do it beside the status, in the attention bar, in Needs You lists, in notifications, and identically across `tasks_list` and `task_get`.
- Legacy browser handoffs stay readable and dismissible until cleared, and answered or cleared handoffs stay cleared after refresh.
- Schema version advances to 78 (`0080_task_wait_messages.sql`).
