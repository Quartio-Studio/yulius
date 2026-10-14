# Git workflow and repository hygiene

`dev` is the integration/default branch. `main` is the production branch (there is
no separate `prod` branch). Both are permanent, protected, and updated through PRs.
These rules apply to features, fixes, documentation, tooling and policy changes.

## Delivery sequence

1. Fetch and create a short-lived branch **with its own worktree** from current `origin/dev`.
2. Implement the change in as many useful commits as needed. Run relevant tests
   during development and `scripts/dev ci` before publishing the completed PR.
3. Open a PR into `dev`. CI tests the proposed merge; keep the branch up to date
   with `dev`. The author checks the diff, evidence and passing required checks,
   then **squash-merges their own PR**. No reviewer approval or waiting period is required.
4. Delete the merged short-lived remote branch. Clean up its local branch/worktree
   after confirming it contains no unpublished work.
5. Wait for the **`yulius/ci` status of the resulting `dev` commit** (Yulius CI runs every
   new `dev` commit). A passing feature PR run does not replace this post-merge test.
   When a dev environment with a deploy command exists, deployment and smoke tests belong
   here too; the standard profile has no manual dev checkpoint, so nobody confirms `dev` by
   hand. A task's work ends here; promotion is a separate step a person asks for.
6. Open a promotion PR **from `dev` into `main`**. Its exact head must have a successful
   `yulius/ci` status: the `yulius-main` ruleset requires it, and Yulius refuses failing,
   running or unreadable checks (`require_checks`). Yulius's merge gate also checks its route
   and title against the branch policy.
7. The promotion PR's author merges it with a **merge commit**, after required checks
   pass. Keep `dev`: only short-lived branches are deleted.
8. `main` then has `dev`'s tested tree. When a production environment exists, verify
   deployment and smoke tests at this point.

No environments are configured yet. Until they are, complete the full sequence
through `main` after each finished feature instead of stopping after integration.
If several tested features accumulate on `dev`, the promotion includes all of them;
list the included PRs. Do not silently omit a feature by cherry-picking to `main`.

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

Squashing turns a feature's working commits into **one meaningful commit on `dev`**,
using the PR title and body. Do not require authors to rewrite every working commit.
Never squash or rebase the long-lived `dev` branch into `main`: a merge commit
preserves common ancestry across repeated promotions, while retaining the already
squashed feature commits. `main` has those feature commits plus promotion commits.
Do not merge `main` back into `dev` just to copy promotion-only merge commits.

`git stash` is a temporary **local** shelf, not history cleanup or a backup. Change
tasks by moving to their existing worktree, not by switching its branch. If a stash
is necessary, name it, include
only your own intended paths, and inspect it before applying. Use `stash apply`
first; drop that exact entry only after verifying restoration. Never auto-stash,
clear stashes, or stash another contributor's work.

## Working commands

Run creation from the permanent dev worktree. Existing changes there stay untouched.
Work in the new directory; never repurpose the dev or main checkout.

```sh
git fetch origin --prune
git worktree add -b feat/task-cancellation ../yulius-worktrees/feat/task-cancellation origin/dev
cd ../yulius-worktrees/feat/task-cancellation
# edit, test and make commits
scripts/dev ci
git push -u origin HEAD
gh pr create --base dev --title 'feat(tasks): add cancellation' --body-file /path/to/pr.md
gh pr checks <number> --required --watch   # the required check is yulius/ci
# Read the final diff, verify checks match the current head, then:
gh pr merge <number> --squash --match-head-commit <verified-head-sha>
```

Do not use `gh pr merge --delete-branch` from a linked worktree: its local cleanup
may try to switch to dev, which already has its own worktree. Repository auto-deletion
removes the remote task branch; perform local cleanup explicitly as described below.

The named author performs the merge. An agent acting on that author's authorized
request follows the same steps; it does not wait for an unsolicited reviewer.
Existing user authorization carries through this delivery workflow. It does not
authorize unrelated deployments, account changes, paid work or messages.

For post-merge evidence, read the `yulius/ci` commit status of **the exact merged SHA** on the
named branch (`gh api repos/{owner}/{repo}/commits/<sha>/status`, or the sandbox's CI page in
Yulius). Do not accept an old green badge or the PR's synthetic merge SHA. A missing, pending,
cancelled or failed status is not success.

Then promote:

```sh
gh pr create --base main --head dev --title 'release: promote dev to main' --body-file /path/to/release.md
gh pr checks <number> --required --watch
gh pr merge <number> --merge --match-head-commit <verified-dev-sha>
# Read the yulius/ci status of the exact resulting main commit and require success.
```

Never pass `--delete-branch` for a promotion. Do not use `--admin` or bypass checks.
A newer `dev` head requires fresh post-merge evidence and updated promotion checks.
Promote only after the `yulius/ci` status of the exact `dev` head is successful; rerun a
failed status only after reading why it failed, never blindly.

## Cleanup, concurrency and recovery

After verifying the PR is merged and its recorded head matches the local branch
exactly, stop branch-owned processes. Inspect tracked, untracked and ignored files
in that worktree; preserve user data and unpublished work. From the permanent dev
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
  then fix/revert through `dev` and promote. Do not bypass the process with a direct
  hotfix to `main`. Any emergency exception requires an explicit recorded user decision.
- Self-review remains required; external review is optional. No required approvals,
  code-owner approval or last-push approval. Resolve actionable comments if present;
  do not fabricate a second reviewer identity.

## Enforcement and maintenance

[dev rules](../../.github/rulesets/dev.json) require squash PRs, linear history,
no force pushes/deletion and a successful `yulius/ci` status (published by the Yulius GitHub
App). Feature branches must be up to date with `dev`. macOS has no Yulius runner: run
`scripts/dev ci` on a Mac before promotion.
[main rules](../../.github/rulesets/main.json) require merge-commit PRs and the same
check; main admits only this repository's `dev`, checked by hand until #223. Both have zero approval requirements and no bypass actors.

`main` does not require `dev` to include main's promotion-only merge commits:
PR CI tests the candidate merge, and main only accepts promotions from dev. Normal
promotion does not change files beyond dev's tested contents. Changing main's
source rule or allowing other writes requires revisiting that assumption.

GitHub settings: default branch `dev`; squash and merge enabled; rebase disabled;
automatic merged-head deletion enabled; squash subject/body from PR title/body.
The JSON files are the desired ruleset configuration, not proof of the live setting.
After authorized settings changes, read back `gh api repos/Quartio-Studio/yulius-v2/rulesets`
and the individual ruleset IDs, and compare to these files. No workflow auto-merges
or holds write credentials. The author drives delivery with `gh`.

GitHub can enforce checks and merge methods; the author's self-review, commit quality
and local cleanup remain contributor responsibilities. Admins can change settings,
so repository policy is not an immutable security boundary.
