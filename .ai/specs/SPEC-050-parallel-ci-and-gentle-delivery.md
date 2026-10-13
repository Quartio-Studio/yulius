---
yulius-task: none
status: done
---

# SPEC-050: tasks run in parallel and finish without obstacles

Status: done
Date: 2026-10-13
Owner: the overnight coordination after the analysis of the `yulius` sandbox's task transcripts
Scope authorization: user requests (F 2026-10-12):
- "take control of the coordination until it's fixed ... '--force-with-lease' is dirty, we want
  everything to be gentle. At the morning I expect every task finished and the system working as
  intended";
- "tasks put in Done doesnt unlock queue";
- "fix this disk thingy";
- "every fix should fix the root cause and not patch. For example if we patch Rust, it'll be
  still a problem in other languages";
- earlier: "agents doesnt seem to work very in parallel".

## Goal

Parallel tasks of a sandbox finish on their own:
- they are not held back by finished tasks, by one CI run at a time, or by agents' builds
  starving CI;
- a refused push does not leave them waiting for nothing;
- disk pressure frees caches nobody is using instead of refusing CI;
- a server restart does not take GitHub away from shells.

Every rule is language-independent.

## Context

On 2026-10-12, 8 parallel tasks ran in the `yulius` sandbox.

- **Done tasks held slots.** Done tasks kept their idle sessions, and those counted against the
  session limit, so queued tasks (#38, #42–#45) never started.
- **Rewritten branches were never pushed.**
  - The briefing allowed rewriting one's own branch with `--force-with-lease`, while Yulius
    pushes plainly at the end of a turn.
  - Agents rebased pushed branches onto `dev`. GitHub refused the pushes as non-fast-forward,
    and the failure only reached people watching the terminal.
  - The agents waited for CI runs that never came (#30, #34, #36).
- **One CI run at a time.**
  - Runs took 5–20 minutes, and each merge sent every other pull request back for a run.
  - Agents' builds ran at the same priority as CI, so CI jobs and e2e tests timed out under
    load.
- **CI was refused on the disk quota while caches sat unused.**
  - Under quota pressure, upkeep pruned only idle tasks' caches.
  - A task waiting for CI keeps a background `yulius wait` shell, so it counted as building
    and kept 10–25 GB.
  - Nothing could be freed, and every CI run was refused.
- **Shells lost GitHub at every restart.**
  - Shells, the Cockpit and editors survive a restart, but their workspace tokens were random
    for each server lifetime.
  - After every deploy their `gh` and Git failed ("could not read Username").

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| SES-101 | A Done task does not count against the live-session limit: its turn ended, no background shell runs, and its goal is achieved, or its plan is complete with the change merged. Its session stays for follow-ups. The queue drains as soon as a task becomes Done. The status view counts the same way. | `supervision::a_done_task_frees_its_slot_for_a_queued_task` |
| DLV-201 | When Yulius cannot push an agent's head, `yulius wait` ends with why and what to do. For a non-fast-forward refusal the fix is to merge the pushed head back in, never to force-push. A later successful push clears it. | `wait_cli::a_push_problem_ends_the_wait`, `delivery.rs` |
| DLV-204 | When the end of a turn delivers nothing (a step is open, the plan or spec is missing or invalid, or work is uncommitted) and the branch has commits to deliver, `yulius wait` says what blocks delivery (#34 waited 42 minutes for a pull request that a still-open step held back). | `delivery::an_undelivered_head_says_what_blocks_it` |
| DLV-202 | Agents never rewrite pushed commits (no rebase, amend or force push, not even `--force-with-lease`), and they bring the integration branch in by merging it. This applies to the briefing, the `git-hygiene` and `om-check-and-commit` skills, the project template and `docs/engineering/git.md`. | `briefing.rs` needles |
| DLV-203 | An armed pull request whose branch no longer contains the integration branch is brought up to date by Yulius, gently: it fetches and merges `origin/<integration>` into the task's worktree, where the repository's merge drivers apply (unlike GitHub's server-side update, which conflicts on a shared changes log), and pushes the result as a plain fast-forward. The armed merge moves to the new head, and its CI run is what decides. This happens only while the agent is idle with nothing running and nothing uncommitted. A real conflict is aborted, leaves the worktree as it was, and names the files for the agent. `delivery_merge` on a branch behind its base arms the merge instead of being refused. | `delivery::an_armed_merge_behind_its_base_is_brought_up_to_date`, `delivery::an_armed_merge_is_kept_current_and_merged_on_green`, `git_ops::merging_the_integration_branch_applies_merge_drivers_and_aborts_conflicts` |
| DLV-205 | Armed merges survive a restart: each is saved in `<data>/armed-merges.json` when armed, moved or decided, and put back at startup for tasks still running (a deploy at 23:26 dropped five armed merges; their agents waited for merges that no longer existed). | `delivery::an_armed_merge_survives_a_restart` |
| DSK-102 | A background shell that only runs `yulius wait` is not a running background shell. Its task reads as waiting, the 30-minute notice ignores it, and disk upkeep may prune the task's caches under pressure. A shell that also runs anything else still counts. | `shells::a_shell_that_only_runs_yulius_wait_does_not_count` |
| CPU-101 | Agent Bash commands run 5 nice levels below the agent, so CI jobs and the agents themselves get the CPU first. `nice` execs the command, so its status is unchanged. | `secrets_redaction` scrub-exec test |
| TRM-101 | A run takes input before anything of it shows: its handle is attached before the welcome screen is published. A writer who took the size while the run was starting sees that size from the first state, never the spawn size first, and the PTY follows. Seen as CI failures on the VM's fast runs: `send_input` right after the banner refused as "not running", and a first state at 120×40. | `terminal_ws::two_viewers_share_the_terminal_and_only_writers_type`, `supervision::a_usage_limit_moves_the_session_to_the_next_account` |
| IDN-101 | A workspace token is derived from a secret in the data directory (`workspace-token.key`, mode 0600), the user and the sandbox. It stays valid across restarts, and a forged token is refused. | `shells::workspace_tokens_stay_valid_across_a_server_restart` |
| CI-106 | Up to `--ci-lanes` runs of a sandbox execute at once (`YULIUS_CI_LANES`, default 2). Each lane past the first has its own job checkouts (`<job>@<lane>`), and the instance-wide job slots still bound the load. Migration 13 drops the one-running-run index. | `ci/mod.rs::lane_tests`, `exec.rs` checkout test, `yulius-store` `ci.rs` |
| CI-107 | Disk upkeep's claim of a sandbox's CI checkouts waits for every lane, and no run starts while it holds. | `ci/mod.rs::lane_tests` |
| CI-109 | Yulius changes worktree registrations (add, move, remove, prune) one at a time. Before, a prune during another add removed that add's half-written registration (#206). | `git_ops.rs::ci_checkouts_created_at_once_all_succeed` |
| CI-110 | A pull request is tested with its own `[[ci.jobs]]` only when it changed them itself (against its merge base with the integration branch), never because the integration branch changed its jobs since: stale branches kept running an apt step `dev` had removed, and two lanes raced for the dpkg lock. | `ci::lane_tests::a_pull_request_runs_its_own_jobs_only_when_it_changed_them` |

## Plan

- [x] Session slots by the Done rule; drain when a task becomes Done
- [x] Push problems in `yulius wait`; gentle git rules everywhere agents read them
- [x] Armed pull requests kept current by Yulius (DLV-203)
- [x] `yulius wait` shells don't count as builds; agent commands under `nice`
- [x] Workspace tokens survive restarts
- [x] CI lanes and serialized worktree registrations

## Evidence

All on macOS:
- **The worktree race:** the checkout test failed in 2 of 5 runs without the lock and passed 6 of 6 with it.
- **Other checks:** the test commands, their results and clippy are listed in the pull request.
