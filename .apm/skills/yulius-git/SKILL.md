---
name: yulius-git
description: Deliver authorized Yulius changes through feature branches, reviewed dev pull requests merged by delivery_merge, and cleanup; promotion to main is a person's. Use for branch creation, PR delivery or Git hygiene.
---

# Deliver a change through Git

Read `AGENTS.md` and `docs/engineering/git.md`; that document owns branch names,
merge methods and commands. Inspect current local/remote state before each stage.
Preserve unrelated edits and never auto-stash someone else's work.

Every local branch must own a worktree, including dev and main. Inspect the existing
mapping first. A task's branch and worktree come from current `origin/dev` (Yulius makes them;
by hand: `git worktree add -b`), and all task work happens there. Never switch a worktree to
another branch. Implement and verify, merge `origin/dev` before the review or for a conflict,
then get an independent review in a fresh session of the exact head and call `delivery_merge`
with that `reviewed_head`: Yulius pushes, opens the pull request into dev and squashes it on
green Yulius CI through its merge queue. Never merge by hand (`gh pr merge` is refused). Yulius
removes a closed task's worktree; a branch made by hand is deleted only after safely removing
its worktree, preserving unpublished commits and tracked/untracked/ignored user data. Keep
permanent dev/main worktrees.

Merge only after the PR answers every review finding (own code or pre-existing,
`docs/engineering/workflow.md`, Review output). Each finding in code or tests the
change added or modified is fixed in the PR, test-first, and never becomes a
follow-up issue; the round limit does not excuse it (keep fixing, split the change
or ask a person), and deferring it means request changes. Only a pre-existing bug,
a flaky test the change did not cause or a cosmetic nit left at the round limit is
filed, saying why it is outside the change.

Require a successful `yulius/ci` status on the exact resulting dev SHA. A task's work
ends there. Promotion (dev → main, a merge commit, never a squash) is a person's, on the CD
page (ADR-0110): a task that needs it asks with `needs_you`. Keep both permanent branches. Do not
claim CI proves a deployed app works.

Distinguish pending, cancelled, failed and absent checks from success. To wait, start
`yulius wait` in the background and end the turn; never poll. Do not restart a run merely
because observation timed out. Reconcile a moved
head and check fresh evidence before merging. Fix real failures through a new dev
branch. Never bypass required checks, use `--admin`, or rewrite a primary branch.

Existing user authorization is sufficient for the requested delivery; do not ask
again at each stage. A local-only request remains local-only, and these instructions
grant no independent permission to publish unrelated work or alter credentials.
Report PRs, merged SHAs, post-merge check results and cleanup status.
