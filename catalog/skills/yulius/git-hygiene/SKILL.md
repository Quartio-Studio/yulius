---
name: git-hygiene
description: Git rules for agents - one task, one branch, one worktree; no stash as a tool; small atomic conventional commits; staying current without rewriting shared history; squash into the integration branch and merge-commit promotion; cleaning up branches and worktrees after merge. Use before any branch, commit, push, rebase, merge or cleanup.
---

# Git hygiene

The repository stays clean enough that anyone (or any agent) can tell what is in flight, what is
done and why. These rules apply to every task; the project's own Git rules, when it has them,
win where they are stricter.

## Branches and worktrees

- **One task = one branch = one worktree**, created together from the current integration
  branch: `git fetch origin --prune && git worktree add -b <type>/<short-name> <path> origin/<integration>`.
  Branch names: lowercase, hyphens, a type prefix (`feat/`, `fix/`, `docs/`, `chore/`,
  `refactor/`, `test/`, `perf/`, `ci/`, `build/`).
- Never switch a worktree to another branch, and never work on the integration or production
  branch directly. Never commit or push to them; changes arrive only through pull requests.
- Never touch another task's worktree, branch or uncommitted files.

## No stash as a tool

Your worktree is yours, so there is nothing to put aside: commit work in progress on your branch,
or move to the other task's worktree. If a stash is truly unavoidable, name it
(`git stash push -m "<why>" -- <your paths>`), apply that exact entry (`git stash apply
stash@{n}`), check the result, then drop that entry only. Never stash or clear others' work.

## Commits

- Small and atomic: one concern per commit, tests with the code they test. Behavior changes and
  refactors in separate commits.
- Conventional messages: `<type>(<scope>): <imperative summary>`, a body that says why when it is
  not obvious, `!` and a migration note for a breaking change.
- Only intended files: read `git status --short` and the diff before every commit. No unrelated
  changes, generated junk, build output, large binaries, local config or secrets; respect and
  extend `.gitignore`.
- Never `--no-verify`, never bypass hooks or checks. A failing hook is a failing gate: fix it.

## Staying current

- Bring your branch up to date with the integration branch the way the project says: rebase
  your own branch before the pull request, or merge the integration branch into it.
- Rewrite only your own, unshared branch history. Force-push only your own branch, and only
  with `git push --force-with-lease`. Never rewrite, force-push or delete the integration or
  production branch, and never rewrite commits someone else built on.
- Resolve conflicts by understanding both sides. Never take "ours" or "theirs" blindly; rerun
  the tests after every conflict resolution.

## Merging and promotion (the default unless the project says otherwise)

- Features and fixes are **squash-merged** into the integration branch (for example `dev`), so
  it has one meaningful commit per pull request.
- The integration branch is **promoted** to the production branch (for example `main`) with a
  **merge commit** (or a fast-forward where the project's delivery profile says so); never squash
  or rebase the integration branch into production, and never cherry-pick around it.
- Merge only on green: the project gate passed on the exact head, the independent review
  approved it, and the branch is up to date (`delivery-pipeline`).

## Cleanup

After the merge: delete the remote branch (if it was not deleted automatically), remove the
worktree, then delete the local branch (`git worktree remove <path>` before
`git branch -D <branch>` for a squash-merged branch, after confirming the pull request merged
with exactly that head). Leave no stale branches, worktrees or tags. Check regularly with the
project's hygiene tool (in Yulius: `repo_hygiene`, and `repo_cleanup` for what is safe to remove).
Never delete a branch or worktree that holds unmerged work.
