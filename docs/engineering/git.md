# Git workflow and repository hygiene

`dev` is the integration/default branch. `main` is the production branch (there is
no separate `prod` branch). Both are permanent, protected, and updated through PRs.
These rules apply to features, fixes, documentation, tooling and policy changes.

## Delivery sequence

1. Fetch and create a short-lived branch from the current `origin/dev`.
2. Implement the change in as many useful commits as needed. Run relevant tests
   during development and `scripts/dev ci` before publishing the completed PR.
3. Open a PR into `dev`. CI tests the proposed merge; keep the branch up to date
   with `dev`. The author checks the diff, evidence and passing required checks,
   then **squash-merges their own PR**. No reviewer approval or waiting period is required.
4. Delete the merged short-lived remote branch. Clean up its local branch/worktree
   after confirming it contains no unpublished work.
5. Wait for the **push CI run on the resulting `dev` commit** to succeed. A passing
   feature PR run does not replace this post-merge test. When a dev environment
   exists, deployment and smoke tests belong here too.
6. Open a promotion PR **from `dev` into `main`**. Its exact head must have a successful
   post-merge `dev` push run; promotion CI verifies this and tests the proposed merge.
7. The promotion PR's author merges it with a **merge commit**, after required checks
   pass. Keep `dev`: only short-lived branches are deleted.
8. Wait for **push CI on the new `main` commit**. Delivery is finished only when it
   passes. When a production environment exists, verify deployment and smoke tests
   at this point. Today CI is the available validation, not evidence of deployment.

No environments are configured yet. Until they are, complete the full sequence
through `main` after each finished feature instead of stopping after integration.
If several tested features accumulate on `dev`, the promotion includes all of them;
list the included PRs. Do not silently omit a feature by cherry-picking to `main`.

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
The branch policy check enforces names, PR titles and valid base/head combinations;
intermediate commit wording is a convention, not an additional blocking check.

## Keep history tidy with squash, not stash

Squashing turns a feature's working commits into **one meaningful commit on `dev`**,
using the PR title and body. Do not require authors to rewrite every working commit.
Never squash or rebase the long-lived `dev` branch into `main`: a merge commit
preserves common ancestry across repeated promotions, while retaining the already
squashed feature commits. `main` has those feature commits plus promotion commits.
Do not merge `main` back into `dev` just to copy promotion-only merge commits.

`git stash` is a temporary **local** shelf, not history cleanup or a backup. Prefer
an isolated worktree when switching tasks. If a stash is necessary, name it, include
only your own intended paths, and inspect it before applying. Use `stash apply`
first; drop that exact entry only after verifying restoration. Never auto-stash,
clear stashes, or stash another contributor's work.

## Working commands

Start with a clean working tree or use a separate worktree. Do not overwrite changes.

```sh
git fetch origin --prune
git switch -c feat/task-cancellation origin/dev
# edit, test and make commits
scripts/dev ci
git push -u origin HEAD
gh pr create --base dev --title 'feat(tasks): add cancellation' --body-file /path/to/pr.md
gh pr checks <number> --required --watch
# Read the final diff, verify checks match the current head, then:
gh pr merge <number> --squash --match-head-commit <verified-head-sha> --delete-branch
```

The named author performs the merge. An agent acting on that author's authorized
request follows the same steps; it does not wait for an unsolicited reviewer.
Existing user authorization carries through this delivery workflow. It does not
authorize unrelated deployments, account changes, paid work or messages.

For post-merge evidence, select the `Engineering checks` workflow run with event
`push`, the named branch and **the exact merged SHA**. Do not accept an old green
badge or the PR's synthetic merge SHA. `gh run list --workflow ci.yml --branch dev
--event push --commit <dev-sha>` identifies the run; `gh run watch <run-id> --exit-status`
waits for its outcome. A missing, cancelled, skipped or failed run is not success.

Then promote:

```sh
gh pr create --base main --head dev --title 'release: promote dev to main' --body-file /path/to/release.md
gh pr checks <number> --required --watch
gh pr merge <number> --merge --match-head-commit <verified-dev-sha>
# Observe the exact resulting main push run and require success.
```

Never pass `--delete-branch` for a promotion. Do not use `--admin` or bypass checks.
A newer `dev` head requires fresh post-merge evidence and updated promotion checks.
A promotion policy check started before the `dev` push run finishes may fail; after
that exact push succeeds, rerun the failed promotion job (not a failed test blindly).

## Cleanup, concurrency and recovery

- Repository auto-deletion removes merged short-lived heads; deletion rules protect
  `dev` and `main`. After merge, fetch with `--prune` and remove only your task's local
  branch/worktree. Squashed heads may fail `git branch -d`: verify the PR is merged,
  its head equals your local branch tip and no later commits exist before using `-D`.
- Do not delete a shared or dirty worktree. Never use `git reset --hard`, `git clean`,
  force pushes or branch deletion as a routine way to resolve conflicts.
- Bring a feature up to date by merging `origin/dev`, resolving conflicts, and rerunning
  affected checks. The final squash keeps `dev` linear. A rebase is optional only
  on your exclusively owned feature branch; coordinate and use `--force-with-lease`.
  Never rewrite `dev` or `main`, and never use plain `--force`.
- Failed post-merge `dev` checks block promotion. Fix on a new branch from `dev` and
  use the same PR/test sequence. Failed `main` checks stop further delivery; diagnose,
  then fix/revert through `dev` and promote. Do not bypass the process with a direct
  hotfix to `main`. Any emergency exception requires an explicit recorded user decision.
- Self-review remains required; external review is optional. No required approvals,
  code-owner approval or last-push approval. Resolve actionable comments if present;
  do not fabricate a second reviewer identity.

## Enforcement and maintenance

[dev rules](../../.github/rulesets/dev.json) require squash PRs, linear history,
no force pushes/deletion and successful `Branch policy`, `quality (ubuntu-latest)`
and `quality (macos-latest)` checks. Feature branches must be up to date with `dev`.
[main rules](../../.github/rulesets/main.json) require merge-commit PRs and the same
checks; `Branch policy` admits only this repository's `dev` and checks its exact
push result. Both have zero approval requirements and no bypass actors.

`main` does not require `dev` to include main's promotion-only merge commits:
PR CI tests the candidate merge, and main only accepts promotions from dev. Normal
promotion does not change files beyond dev's tested contents. Changing main's
source rule or allowing other writes requires revisiting that assumption.

GitHub settings: default branch `dev`; squash and merge enabled; rebase disabled;
automatic merged-head deletion enabled; squash subject/body from PR title/body.
The JSON files are the desired ruleset configuration, not proof of the live setting.
After authorized settings changes, read back `gh api repos/Quartio-Studio/yulius/rulesets`
and the individual ruleset IDs, and compare to these files. No workflow auto-merges
or holds write credentials. The author drives delivery with `gh`.

GitHub can enforce checks and merge methods; the author's self-review, commit quality
and local cleanup remain contributor responsibilities. Admins can change settings,
so repository policy is not an immutable security boundary.
