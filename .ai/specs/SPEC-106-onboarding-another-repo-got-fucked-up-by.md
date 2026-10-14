---
yulius-task: eb16ec99-5255-4bc3-aa8c-048b492074aa
status: in-progress
---

# SPEC-106: onboarding survives the trust prompt and a lost onboarding task

Status: in-progress
Date: 2026-10-14
Owner: task #91
Scope authorization: the person's request (task #91): onboarding Quartio-Studio/studio got
stuck; "someone typed in the session before the goal", and after stopping the task the page said
"The onboarding task is missing". "Fix onboarding and make sure the repos that got affected (like
studio in this example) will get fixed." Issue #425.

## Goal

Onboarding a repository runs to its end without a person getting stuck. Its agent starts without
Claude Code's folder-trust prompt. If a trust prompt does appear, answering it does not cost the
task its goal. A sandbox whose onboarding task is gone can start onboarding again. Sandboxes
already stuck this way heal by themselves.

## Context

The symptoms come from two screenshots. The first shows task #1 "Prepare this repository for
Yulius" in the studio sandbox, stopped on Claude Code's "Quick safety check … This folder
pre-approves 21 tool permissions in .claude/settings.json". The second shows the studio page after
the stop. It reads "The onboarding task is missing, so onboarding cannot finish by itself", its
only way out is an admin API call, and the sidebar says "No active tasks".

Root causes, each reproduced with Claude Code 2.1.292 and the code on `dev`:

1. **Trust is looked up under the main repository.** Yulius runs each task in a git worktree,
   `<sandbox>/worktrees/<task>`, of the sandbox clone `<sandbox>/repo`. `ensure_claude_config`
   (`yulius-harness/src/claude_config.rs`) marks only the worktree trusted. When the repository's
   `.claude/settings.json` pre-approves permissions, Claude Code looks up the trust of a worktree
   under its main repository's path. Measured in a pty with an isolated config dir:

   | Trusted paths | `.claude/settings.json` allow list | Prompt shown |
   | --- | --- | --- |
   | worktree | yes | **yes** |
   | worktree and main repository | yes | no |
   | main repository | yes | no |
   | worktree | none | no |
   | plain repository (no worktree) | yes | no |

   Most repositories have no allow list, and Yulius's own main repository was trusted long ago by
   a session in its clone, which is why this only hit a fresh onboarding.
2. **Answering the prompt cost the goal.** `spawn_goal` (`yulius-app/src/supervisor.rs`) leaves a
   person's draft alone: if anything was typed since the run attached, no goal is typed and the
   task is flagged (SPEC-068 TYP-003, #252). The keys that answered the trust prompt count as such
   a draft. The task therefore never got its goal and needed a person.
3. **A lost onboarding task blocks the sandbox for good.** Deleting a task clears
   `sandboxes.onboarding_task_id` (`yulius-store/src/tasks.rs`) but leaves the sandbox `preparing`.
   `refresh_onboarding` returns early without a task, and `onboarding_task` creates one only in
   `pending`. An archived onboarding task drops out of `?archived=false`, which the gate
   (`taskview.js`) reads as missing. Only `POST …/onboarding/skip` remained, and that skips
   preparation rather than doing it.

**Why the tests missed it:**
- The live trust check (`real_claude_tui_reaches_the_prompt_inside_ptyhost`) uses a repository
  without an allow list.
- The draft test (#252) has no dialog before the prompt.
- The gate's "missing" case was tested as a message, not as a state to leave.

**What else the cause touches.**
- Every task and Cockpit session in a worktree of a repository whose `.claude/settings.json`
  pre-approves tools, not only onboarding.
- Any goal task whose session shows the trust prompt.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| OTR-001 | `ensure_claude_config` trusts the worktree and, when the worktree is a linked git worktree (its `.git` is a `gitdir:` file), the main repository too: the parent of the common git directory when it is named `.git`, else the common directory (a bare repository). An unreadable or malformed `.git` file trusts the worktree alone, as before. | `claude_config` unit tests |
| OTR-002 | While Claude Code shows its folder-trust prompt, the goal is not typed. The keys a person types to answer it are not a draft. Once the prompt is gone and the TUI is ready, the goal is typed as usual. The wait is bounded by the opening timeout (5 min). A prompt still shown then flags the task and types nothing. | `first_message::a_trust_prompt_answered_before_the_goal_keeps_the_goal` |
| OTR-003 | A `pending` or `preparing` sandbox whose onboarding task does not exist (deleted, or never recorded) is moved to `pending`, so the page offers Start onboarding again, which creates a new onboarding task. Recorded in the audit log as `system`. | `projects::a_preparing_sandbox_whose_onboarding_task_is_gone_returns_to_pending` |
| OTR-004 | Archiving the onboarding task of a blocking sandbox is refused (409). The message names the ways out: continue the task, or delete it to start onboarding again. An onboarding task archived before this change is restored to the task list (OTR-005). | `projects::the_onboarding_task_is_not_archived_while_onboarding_blocks` |
| OTR-005 | The repairs (OTR-003, and restoring an archived onboarding task) run at server start for every sandbox, right after a task is deleted, and on every onboarding refresh. That is how sandboxes already stuck this way (studio) heal with the deploy. | `projects::stuck_onboarding_heals_at_server_start` |

## Plan

- [ ] Spec and issue for the stuck onboarding
- [ ] Agents trust the main repository of their worktree
- [ ] Goal is never typed into Claude Code's trust prompt
- [ ] A preparing sandbox without its onboarding task returns to Start onboarding
- [ ] Stuck sandboxes heal at startup and on task delete
- [ ] Docs, review and delivery

## Acceptance criteria

- With Claude Code 2.1.292, a worktree of a repository that pre-approves permissions in
  `.claude/settings.json`, prepared by `ensure_claude_config`, starts at the prompt (no "Quick
  safety check").
- A goal task whose agent shows a trust prompt that a person answers gets `GOAL SET` and is not
  flagged.
- A `preparing` sandbox whose onboarding task was deleted reads `pending` after the delete. Its
  Start onboarding creates and starts a new onboarding task.
- Archiving the onboarding task of a preparing sandbox answers 409, and the task stays listed.
- After a server start, a sandbox left `preparing` without its task reads `pending`, and one with
  an archived onboarding task lists it again.

## Test plan

- Unit: `yulius-harness` `claude_config` tests for a linked worktree, a bare main repository and
  a malformed `.git` file.
- Supervisor (fake agent in ptyhost): a new fake-agent `--trust-prompt` draws a trust prompt and
  takes one answer before its SessionStart hook. The test answers it as a person and expects the
  goal, with no flag. It fails before the fix (the task is flagged "Someone typed …").
- Integration (`crates/yulius/tests/projects.rs`): deleting and archiving the onboarding task of
  a preparing sandbox, and a store left stuck being healed by the startup sweep.
- Live, by hand: Claude Code 2.1.292 in a pty with the generated config (see Evidence).
- Full gate: `scripts/dev ci`.

## Decisions

- [ADR-0086](../../docs/decisions/0086-a-lost-onboarding-task-returns-the-sandbox-to-pending.md):
  a lost onboarding task returns the sandbox to `pending` instead of skipping preparation.
- Trust the main repository rather than answer the prompt by keystrokes. Yulius already decides
  to trust each worktree it creates, and the main clone is Yulius's own (same sandbox, same
  owner). Answering a dialog by its screen text would break with each Claude Code release.
- The trust-prompt wait (OTR-002) is kept as a guard: a future Claude Code may look trust up
  elsewhere again. Its detection reads two phrases of the dialog ("Yes, I trust this folder" for
  2.1.x and "Do you trust the files in this folder?" for older releases).
