---
name: yulius-git
description: Deliver authorized Yulius changes through feature branches, self-merged dev PRs, verified promotion to main and cleanup. Use for branch creation, PR delivery, promotion or Git hygiene.
---

# Deliver a change through Git

Read `AGENTS.md` and `docs/engineering/git.md`; that document owns branch names,
merge methods and commands. Inspect current local/remote state before each stage.
Preserve unrelated edits and never auto-stash someone else's work.

Use a short-lived typed branch from current `origin/dev`. Implement and verify,
then push and open its PR into dev when the task authorizes delivery. The author
self-reviews and squash-merges after all required checks pass, with the verified
head SHA; no external approval or waiting period is required. Delete only the
merged task branch, preserving unpublished commits and dirty worktrees.

Require successful push CI on the exact resulting dev SHA before promotion. Open
dev → main with a release title; after promotion checks pass, author-merge with a
merge commit, never squash. Keep both permanent branches. Wait for successful push
CI on the resulting main SHA before reporting delivery complete. No deployment
environment exists yet; do not claim CI proves a deployed app works.

Distinguish pending, cancelled, failed and absent checks from success. Poll live
run IDs; do not restart merely because observation timed out. Reconcile a moved
head and check fresh evidence before merging. Fix real failures through a new dev
branch. Never bypass required checks, use `--admin`, or rewrite a primary branch.

Existing user authorization is sufficient for the requested delivery; do not ask
again at each stage. A local-only request remains local-only, and these instructions
grant no independent permission to publish unrelated work or alter credentials.
Report PRs, merged SHAs, post-merge check results and cleanup status.
