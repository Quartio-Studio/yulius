# 0419. Antigravity Stop settles the turn, delivery computes fresh PR conflicts, and armed green PRs have a stall bound

## Status

Accepted for [SPEC-580](../../.ai/specs/SPEC-580-an-armed-green-pr-merges-when-its-agent.md).

## Context

Observed on the Quartio instance (`build 3fc0e5976`, 2026-11-21) across two Antigravity tasks (`Quartio-Studio/quartio-v2` task #338 PR #1498 and `Quartio-Studio/yulius` task #563 PR #1568):

1. Antigravity CLI sets `fullyIdle: false` on `Stop` whenever any background command is alive — including `yulius wait`. `App::hook_stop` skipped `clean_stop` whenever `fullyIdle: false`, leaving `activity = Working` (`shows = working`) for an agent idle at its prompt and blocking the merge queue's "turn ended with everything committed" condition (`decidable_in`). Meanwhile, `yulius-ptyhost::shells` ignored PTY-hosted background commands (such as Antigravity's `run_command` shells on a child PTY) and `settle_clean_turn` did not call `runtime.end_turn()` or update `turn_closed_ms`, leaving `runtime.turn_in_progress()` true on Antigravity turns.
2. `delivery_view` hardcoded `pr.conflicts = false` instead of calling `App::pull_conflicts`, `pull_conflicts` could trust a stale local clone over GitHub's `mergeable = Some(false)`, and `merge_armed_task` never dropped a conflicting armed green PR before waiting behind the integration branch.
3. No bounded watch existed for an armed green PR stuck outside a merge train: `pr_watch`'s 60-minute `STALL_AFTER_MS` required `!facts.working` and only appended a label without escalating to a person.

## Decision

1. **Antigravity Stop with background probes and turn close**: `yulius-ptyhost::shells` tracks each process's controlling terminal (`tty_nr` from `/proc/<pid>/stat` on Linux) and distinguishes wait-only shells from real non-wait PTY-hosted background commands, ignoring Yulius's own `yulius hook` / `yulius mcp` helpers. `TaskRuntime` tracks active `/internal/agent/wait` RPCs (`active_waits`). On `Stop` with `fullyIdle: false`, `hook_stop` probes live background processes + `active_waits`: when only `yulius wait` is running (`shells == 0 && waits > 0`), Yulius clears `background_agents` / `background_shells`, records `background_waits`, and settles cleanly (`Activity::Waiting`, `yulius_wait = true`, merge queue decidable); when non-wait background work runs, Yulius settles `Activity::Waiting` while synchronously seeding `background_shells >= 1` (`background_agents = 1`) so SPEC-528 never flickers `NeedsYou` and the merge queue holds until non-wait work ends. `settle_clean_turn` calls `runtime.end_turn()` and advances `turn_closed_ms` for every harness.
2. **Fresh PR conflict reporting and immediate arm drop**: `delivery_view` populates `pr.conflicts` via `App::pull_conflicts`. When `local_conflict` returns `Some(false)` while GitHub reports `mergeable == Some(false)`, `pull_conflicts` refreshes the sandbox clone (`dlv_refresh_clone`) and re-runs `local_conflict` — reporting real conflicts immediately while preserving SPEC-491 M1 union-driver clean merges (falling back to `true` only if the refresh itself fails). `decidable_in` treats a cached conflict as decidable, and `merge_armed_task` drops a conflicting armed green PR immediately with non-destructive conflicting file names and resolution instructions.
3. **Bounded armed-green PR stall check-in and Needs You escalation**: `pr_watch` tracks an independent wait clock per `(pr, armed_head)` whenever a PR is armed and green outside an active merge train (even while `working == true`). After `ARMED_CHECK_IN_AFTER_MS` (10 min), Yulius sends an actionable check-in naming the PR, head, integration branch, and blocking reason. If the PR still does not progress after `ARMED_NEEDS_YOU_AFTER_MS` (10 min after the check-in, or 20 min total if check-in delivery is blocked), Yulius escalates via SPEC-578's `NeedsYouRecord` (`tasks.needs_you` + `publish_task` -> `needs_you_notice`, `tasks_list`/`task_get`, `task_wait_messages`, and audit `task.needs_you.armed_stalled`). The escalation auto-clears via `end_needs_you_asked_at` once the PR enters a train, changes head, disarms, stops being green, or merges.

## Alternatives

- Treating `fullyIdle: false` identically to `fullyIdle: true` without probing background processes would regress SPEC-528 by showing Needs You whenever a real Antigravity subagent or long-running background shell ran alongside open steps, or allowing a merge while background builds still modified the worktree.
- Trusting GitHub's `mergeable == Some(false)` unconditionally without refreshing and re-running `git merge-tree` would break SPEC-491 M1 union-driver files (such as `CHANGES.md` / `docs/changes.md`).
- Relaxing exact-head or merge-order queue safety rules would weaken merge-train guarantees and is rejected.

## Consequences

- Antigravity tasks parked on `yulius wait` show `activity = waiting` with `yulius_wait = true` (never `working`), close `turn_in_progress`, and merge through the merge queue on parity with Claude Code and Codex.
- Conflicting armed PRs report `pr.conflicts = true` in `delivery_status` / `task_delivery` and drop their arm immediately with conflicting file names rather than waiting silently.
- Any armed green PR that stalls outside a merge train gets an agent check-in at 10 minutes and escalates to a person via SPEC-578 Needs You at 20 minutes, auto-clearing once it moves or merges.
