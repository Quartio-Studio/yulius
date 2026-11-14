# Git workflow and repository hygiene

`dev` is the integration/default branch. `main` is the production branch (there is
no separate `prod` branch). Both are permanent, protected, and updated through PRs.
These rules apply to features, fixes, documentation, tooling and policy changes.

## Delivery sequence

Yulius develops itself: a task's agent works in Yulius, which pushes, opens the pull request and
merges it. Its steps:

1. Yulius creates the task's short-lived branch **with its own worktree** from current
   `origin/dev`.
2. Implement the change in as many useful commits as needed. Run relevant tests during
   development and `scripts/dev ci` before the review. Merge `origin/dev` before the review
   or for a conflict (never rebase), and run the tests again.
3. An **independent review in a fresh session** reads the exact head (a read-only reviewer given
   the diff range and the spec, skill `om-code-review`); every finding is answered
   ([workflow](workflow.md#review-output)). A commit after the approval needs a look at it too;
   a clean merge of `dev` after it (no conflict resolved) keeps the approval.
4. Call `delivery_merge` with that `reviewed_head`. Yulius pushes the branch, opens the pull
   request into `dev` and merges it (squash) on green Yulius CI at the exact head, through its
   merge queue, which tests a pull request that is behind on top of the current `dev`. To wait,
   start `yulius wait` in the background and end the turn; Yulius tells the session how it ended.
   Merging by hand (`gh pr merge`) is refused: it skips the queue and the merge's own checks.
5. After the merge, Yulius deletes the remote branch and, once the task is closed, its worktree.
   Remove what the task itself made (review worktrees with `git worktree remove`, scratch
   files); `repo_hygiene` lists, and `repo_cleanup` removes, stale branches and the worktrees
   of merged, archived or deleted tasks.
6. Yulius CI runs every new `dev` commit; a task's work ends at `dev`. Deploying dev is a task's
   only when it asks (`cd_deploy`).
7. **Promotion is a person's** ([ADR-0110](../decisions/0110-only-a-person-deploys-production.md)):
   an administrator promotes on the CD page, typing the project's slug. Yulius then opens the
   promotion pull request from `dev` into `main` and merges it with a **merge commit** when the
   exact `dev` head has a successful `yulius/ci` status (the `yulius-main` ruleset requires it);
   otherwise the deploy is refused with why, and the person deploys again later.
   An agent never promotes; a task that needs production asks a person with `needs_you`.

If several tested features accumulate on `dev`, a promotion includes all of them. Never
cherry-pick to `main`.

## One branch, one worktree

Every local branch MUST have its own registered Git worktree, including `dev` and
`main`. Create a branch and its worktree together with `git worktree add -b`.
Never switch a worktree between branches or create a branch without its worktree.
All edits, commits and local checks for a branch run inside that branch's worktree.

Keep the original checkout assigned to `dev`. Put linked worktrees in the sibling
`<repo>-worktrees/<branch>` directory, preserving the slash in typed branch names:

```text
yulius/                                  dev (permanent)
yulius-worktrees/main/                    main (permanent)
yulius-worktrees/feat/task-cancellation/   feat/task-cancellation (temporary)
```

Before starting work, inspect `git worktree list` and
`git for-each-ref --format='%(refname:short) %(worktreepath)' refs/heads/`.
Reuse a branch's existing worktree if it already exists; do not duplicate or force
its checkout. Attach any existing local branch with a blank worktree path before
using it. For example, `git worktree add ../yulius-worktrees/main main` attaches an
existing main branch; if only `origin/main` exists, use
`git worktree add --track -b main ../yulius-worktrees/main origin/main` instead.

Remote-tracking refs are not additional local branches. Detached CI checkouts have
no branch to assign. This is a local working-directory rule; GitHub cannot inspect
an author's worktree layout. Verify it locally, not with a pretend hosted check.

Worktrees share Git objects and refs, but keep source, indexes and working files
separate. Do not share a mutable build directory, `.env`, secrets or runtime data
between branches. A verified, identically pinned tool-binary cache may be reused.
Keep permanent worktrees for their lifetime; remove a task worktree only after its
branch merges and the cleanup checks below pass.

## Names

Use lowercase ASCII and hyphenated words:

```text
<type>/<short-description>
<type>/<issue-number>-<short-description>   # when an issue exists
```

Allowed types: `feat`, `fix`, `refactor`, `perf`, `docs`, `test`, `build`, `ci`,
`chore`, `revert`. Examples: `feat/42-task-cancellation`, `fix/event-replay`,
`docs/provider-setup`, `chore/git-workflow`. One branch represents one cohesive
outcome. No personal prefixes, nested slashes, spaces, underscores or ticket-only names.
The description must contain a letter; keep the complete branch name at most 80 characters.

PR titles and ordinary commit subjects use:

```text
<type>[optional-scope][!]: <imperative description>
feat(tasks): add cancellation
fix(events): preserve replay order
chore(git): define delivery workflow
```

Scopes use lowercase letters/digits/hyphens. `!` marks a breaking contract change;
explain migration in the body. PR titles are limited to 100 characters. Promotion
PRs use `release: promote dev to main` (optionally append a short release label).
The branch policy defines names, PR titles and valid base/head combinations. Yulius's merge
gate checks it before every merge it makes (task PRs, armed merges, the Git page, promotion
PRs) and refuses a violation with the rule in words; fix the title (`gh pr edit`) or the branch
and merge again. It is configured in `yulius.toml` `[branch_policy]` (`preset = "standard"` or
`"none"`, optional `types`), read from `dev` (SPEC-069, ADR 0051).
A task PR Yulius opens is titled with the task's title after the type and scope of its most
significant commit (feat, fix, perf, refactor, revert, build and ci, test, docs, style, chore), so
not the spec commit's `docs` when the branch holds a feature or a fix; before it merges, Yulius
renews a title it gave from the final commits, and keeps a title you wrote (SPEC-069 BPL-008).
Intermediate commit wording is a convention, not an additional blocking check.

## Keep history tidy with squash, not stash

Squashing turns a feature's working commits into **one meaningful commit on `dev`**, using the PR
title and body (Yulius squashes when it merges). Do not require authors to rewrite every working
commit. Never squash or rebase the long-lived `dev` branch into `main`: a merge commit preserves
common ancestry across repeated promotions, while retaining the already squashed feature commits.
`main` has those feature commits plus promotion commits. Do not merge `main` back into `dev` just to
copy promotion-only merge commits.

No `git stash` in a task's worktree: the stash stack is shared by every worktree of the
clone, so another session may pop your entry. Set work aside in a temporary commit instead.
Change tasks by moving to their existing worktree, not by switching its branch.

## Working commands

Yulius creates a task's branch and worktree; by hand, run creation from the permanent dev
worktree. Existing changes there stay untouched. Work in the new directory; never repurpose the
dev or main checkout.

```sh
git fetch origin --prune
git worktree add -b feat/task-cancellation ../yulius-worktrees/feat/task-cancellation origin/dev
cd ../yulius-worktrees/feat/task-cancellation
# edit, test and make commits
scripts/dev ci
```

Then the review and `delivery_merge` (above). For evidence after the merge, read the `yulius/ci`
status of **the exact merged SHA** (`ci_status`, the sandbox's CI page, or
`gh api repos/{owner}/{repo}/commits/<sha>/status`). Do not accept an old green badge or the
pull request's synthetic merge SHA. A missing, pending, cancelled or failed status is not
success; a failed one is read before anything is run again (`ci_rerun` only for a failure the
code did not cause).

Existing user authorization carries through this delivery workflow. It does not authorize
production, unrelated deployments, account changes, paid work or messages.

## Cleanup, concurrency and recovery

In Yulius a task's worktree is Yulius's (it removes it once the task is closed); the steps below are
for a branch and worktree made by hand. After verifying the PR is merged and its recorded head
matches the local branch exactly, stop branch-owned processes. Inspect tracked, untracked and
ignored files in that worktree; preserve user data and unpublished work. From the permanent dev
worktree, remove the task worktree **before** deleting the local branch:

```sh
git fetch origin --prune
git worktree remove ../yulius-worktrees/feat/task-cancellation
git branch -d feat/task-cancellation
```

Do not force-remove a dirty/locked worktree. If Git refuses removal, inspect the
reason and preserve the data. If only known disposable build/cache artifacts block
removal, remove those specific artifacts first. Squash merging may make `branch -d`
refuse deletion even though the PR was merged; only after the checks above, use
`git branch -D <exact-merged-task-branch>`. If remote auto-deletion did not occur,
verify the same merged-head evidence before `git push origin --delete <task-branch>`.
Never delete either primary branch or its worktree.

Update each clean permanent worktree in place after merges, using
`git -C <dev-worktree> merge --ff-only origin/dev` and
`git -C <main-worktree> merge --ff-only origin/main` after fetching. Do not switch
branches or use forced ref updates to refresh a checked-out branch.

- Do not delete a shared or dirty worktree. Never use `git reset --hard`, `git clean`,
  force pushes or branch deletion as a routine way to resolve conflicts.
- Bring a feature up to date by merging `origin/dev`, resolving conflicts, and rerunning
  affected checks. The final squash keeps `dev` linear. Never rebase, amend or force-push
  commits once they are pushed (Yulius pushes task branches with a plain push, so a rewritten
  branch can no longer be pushed), and never rewrite `dev` or `main`.
- Failed post-merge `dev` checks block promotion. Fix on a new branch from `dev` and
  use the same PR/test sequence. Failed `main` checks stop further delivery; diagnose,
  then fix/revert through `dev`; a person promotes the fix. Do not bypass the process with a
  direct hotfix to `main`. Any emergency exception requires an explicit recorded user decision.
- The independent review in a fresh session approves the exact head before `delivery_merge`;
  GitHub requires no approvals, code-owner approval or last-push approval. Resolve actionable
  comments if present; do not fabricate a second reviewer identity.

## Enforcement and maintenance

[dev rules](../../.github/rulesets/dev.json) require squash PRs, linear history,
no force pushes/deletion and a successful `yulius/ci` status (published by the Yulius GitHub
App). Feature branches must be up to date with `dev`. macOS has no Yulius runner: run
`scripts/dev ci` on a Mac before promotion.
[main rules](../../.github/rulesets/main.json) require merge-commit PRs and the same
check; main admits only this repository's `dev`, which Yulius's merge gate checks. Both have zero
approval requirements and no bypass actors.

`main` does not require `dev` to include main's promotion-only merge commits:
PR CI tests the candidate merge, and main only accepts promotions from dev. Normal
promotion does not change files beyond dev's tested contents. Changing main's
source rule or allowing other writes requires revisiting that assumption.

GitHub settings: default branch `dev`; squash and merge enabled; rebase disabled;
automatic merged-head deletion enabled; squash subject/body from PR title/body.
The JSON files are the desired ruleset configuration, not proof of the live setting.
After authorized settings changes, read back `gh api repos/Quartio-Studio/yulius/rulesets`
and the individual ruleset IDs, and compare to these files. No GitHub workflow merges or
holds write credentials: Yulius merges through `delivery_merge` and its merge queue.

GitHub can enforce checks and merge methods; the review, commit quality and local cleanup
remain contributor responsibilities. Admins can change settings,
so repository policy is not an immutable security boundary.
