# Git workflow and repository hygiene

`dev` is the integration/default branch. `main` is the production branch (there is
no separate `prod` branch). Both are permanent, protected, and updated through PRs.
These rules apply to features, fixes, documentation, tooling and policy changes.

## Delivery sequence

Yulius develops itself: a task's agent works in Yulius, which pushes, opens the pull request and
merges it. Its steps:

1. Yulius creates the task's short-lived branch **with its own worktree** from current
   `origin/dev`.
2. Implement the change in as many useful commits as needed, running the checks it touches
   (`delivery-pipeline`). Merge `origin/dev` before the review or for a conflict, and run the
   tests it touches again.
3. An **independent review in a fresh session** reads the exact head (a read-only reviewer given
   the diff range and the spec, skill `om-code-review`). Every finding is answered
   ([workflow](workflow.md#review-output)). A commit after the approval needs a look at it too; a
   clean merge of `dev` after it (no conflict resolved) keeps the approval.
4. Call `delivery_merge` with that `reviewed_head`. Yulius pushes the branch, opens the pull
   request into `dev` and merges it (squash) on green Yulius CI at the exact head, through its
   merge queue, which tests a pull request that is behind on top of the current `dev`.
   Merging by hand (`gh pr merge`) is refused: it skips the queue and the merge's own checks.
5. After the merge, Yulius deletes the remote branch and, once the task is closed, its worktree.
   Remove what the task itself made (review worktrees with `git worktree remove`, scratch
   files); `repo_hygiene` lists, and `repo_cleanup` removes, stale branches and the worktrees
   of merged, archived or deleted tasks.
6. Yulius CI runs every new `dev` commit; a task's work ends at `dev`. `dev` has the merge-first
   staging target `[cd.dev]` ([ADR-0436](../decisions/0436-yuliuss-full-suite-runs-after-the-merge-and-promotion-comes-from-verified-staging.md)).
7. **Promotion is a person's** ([ADR-0110](../decisions/0110-only-a-person-deploys-production.md)):
   an administrator promotes on the CD page, typing the project's slug. Yulius then opens the
   promotion pull request into `main` from the confirmed, verified dev revision. When a newer
   revision is still being checked, CD can use a release ref pinned to the earlier verified SHA.
   The `yulius-main` ruleset requires successful `yulius/ci` on the selected revision for the
   **merge commit**. A refused deploy explains which evidence is missing.

If several tested features accumulate on `dev`, a promotion includes all of them. Never
cherry-pick to `main`.

## Worktrees

Yulius creates each task's branch and worktree. By hand, follow the bundled skill `git-hygiene`
(one worktree per local branch, created together with `git worktree add -b`), and keep the
original checkout on `dev` with linked worktrees in the sibling `<repo>-worktrees/<branch>`
directory:

```text
yulius/                                  dev (permanent)
yulius-worktrees/main/                    main (permanent)
yulius-worktrees/feat/task-cancellation/   feat/task-cancellation (temporary)
```

Do not share a mutable build directory, `.env`, secrets or runtime data between worktrees; a
verified, identically pinned tool-binary cache may be reused.

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

## Keep history tidy with squash

Squashing turns a feature's working commits into **one meaningful commit on `dev`**, using the PR
title and body (Yulius squashes when it merges). Do not require authors to rewrite every working
commit. Never squash or rebase the long-lived `dev` branch into `main`: a merge commit preserves
common ancestry across repeated promotions, while retaining the already squashed feature commits.
`main` has those feature commits plus promotion commits. Do not merge `main` back into `dev` just to
copy promotion-only merge commits.

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

### Post-merge machine evidence

Open **Delivery → Verification evidence** on the task or call `delivery_status`. The link is scoped to the task and the full landed SHA. Use the newest applicable attempt for each required check. Read its reference, commit, environment and individual verdicts. When `delivery_status` lists older attempts, call it again with the supplied `merged_sha` and `before`. Pass `run_id` to get a link pinned to the attempt you cite. In the browser these are `?before=<next_before>` and `?run_id=<id>`. A green PR or merge-train verdict can explain the merge. Integration CI requires the branch run at the landed SHA. A verified deployment requires a passed CD deploy of that SHA to the named environment with passed deploy and verify commands. A rollback drill, a promotion, a missing command, or a command run that awaited manual confirmation supplies different evidence.

When a project accepts task evidence, finish its verification-only step with `step_update`, citing the link and run IDs in the note and closing summary. For example, “Integration tests passed in run A. Security scan skipped. Staging deploy and smoke check passed in run B.” Only close a step whose required checks actually passed. Keep pending checks open, report failed or skipped checks as recorded, and obtain required manual or hardware evidence separately.

Commit required specs, changelogs and source evidence through the project's usual review flow. The repo handoff is to say which machine checks may live in the task record and which artifacts must be committed. A source spec that still declares work keeps the task open. Where the project requires a committed result, or the checks uncover a code or test change, deliver that work through a reviewed PR.

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
  affected checks. The final squash keeps `dev` linear; never rewrite `dev` or `main`.
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
check. A promotion comes from this repository's `dev` or a release ref pinned to its verified history by production CD ([ADR-0425](../decisions/0425-pin-a-production-deploy-to-verified-dev.md)). Both have zero
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
