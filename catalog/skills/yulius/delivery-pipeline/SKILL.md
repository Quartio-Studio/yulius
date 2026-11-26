---
name: delivery-pipeline
description: The one autonomous path every feature and fix takes, from a ready issue to a merge on green - spec, slice-by-slice implementation, independent review in a fresh session, fixes, gates, a small pull request, merge on green, cleanup - and how CD (deployment) follows it. Use for any change that will be delivered, and whenever you wonder what comes next.
---

# Delivery pipeline

Every feature and fix takes the same path. There is no human acceptance step: the merge tool
checks CI, the review and freshness, and the independent review checks the rest, so you decide,
record why, and move on.

```text
ready issue → spec → slices (test first) → local gates → independent review → fixes
  → pull request → merge on green → cleanup
CD, separately: deploy dev and verify (when asked, or nightly), its rollback drilled · production: a person promotes and deploys
Merge-first staging: quick checks → merge → full gate on the merged revision → cd_deploy once → staging verified
```

Research, questions about the product, documentation and small chores skip the spec; when they
change the repository they still follow the Git and gate rules, and their merge still needs the
independent review `delivery_merge` asks for.

## 0. Ready

Work starts from one concern with acceptance criteria. A GitHub issue is **ready** when it has:

- exactly one `type:` label (bug, enhancement, docs, chore, performance), one `priority:` label
  (high, medium, low), at least one `area:` label, and `risk: high` when it touches
  authentication, permissions, data isolation, migrations, money or a shared contract;
- a summary, current and expected behavior, what is out of scope, and a checklist of acceptance
  criteria.

Missing labels are created with the issue, never skipped (in Yulius: `issue_create`). A bug you
find in code the change did not introduce, and a flaky test the change did not cause, become their
own issues instead of widening the current task, named in the task's summary; a finding in the
change's own code never does (`om-code-review`). A defect the task's own request reports, also one
asked as a question ("why is this broken?", a screenshot of a fault) or confirmed while answering
it, is fixed in the task, never filed instead (its issue, if one is opened, closes with the fix's
`Fixes #N`): the plan grows the fixing steps, or the task asks the person whether to fix it when
the fix is clearly beyond the request. A true question (how something
works, what it would cost) is answered without a pull request.

Before designing, know the product: read the docs index (AGENTS.md, product summary,
architecture, decisions, earlier specs, recent changes), the parallel and previous tasks, and
search earlier sessions for how the area was built. Extend what exists; never build a second
version of it.

Look at the neighbours before work that may overlap theirs: in Yulius `tasks_list`, then
`task_get` on each task near your change, shows its plan with notes, its pull request and
delivery stage, its claimed paths, what it waits for and its screen. If one already does the
work, extend its result after it merges or take a different slice; if one touches the same files,
wait for its pull request or claim disjoint paths. Never raise a Needs you asking a person to
relay a status you can read there. The view is read-only: tasks never message or steer each
other, and only a person, through their Superagent, writes to a task.

## 1. Spec (features and fixes)

Use `om-spec-writing`. The Plan is steps of **similar effort**, each a slice that leaves the
project working and is named by what it delivers ("Store drafts per user", not "Implement"):
3-12 for one pull request. It holds **all the work you know of**: when the task will deliver
several pull requests (the slices of an audit, the phases of a migration), every one of them is
a step from the start, at most 60, and a step is added later only for work found later. Those
steps are the task's progress: 9 of 10 done must mean about 90 %, and "6 of 6" must mean that
nothing known is left. A step is done once nothing of it remains: the review step when the
reviewer approved, not when its first round is answered. A fix's spec can be short: symptom,
root cause (`om-root-cause`), the failing test, the fix.

## 2. Slices, test first

Work on your own branch and worktree (`git-hygiene`). For each step: mark it in progress, write
the test that fails without the change and watch it fail (`test-driven-development`), make it
pass with the smallest change, refactor, mark the step done. One step at a time; never tick a
batch at the end. Behavior and refactoring go in separate commits; docs change in the same pull
request as the behavior they describe; risky schema changes are expand → migrate → contract.

## 3. Local gates

While iterating, run the checks your change touches. The whole gate is Yulius CI's job: it tests
the head while the reviewer reads. Do not run the whole gate locally before a review or between
rounds, except to reproduce a CI failure or when CI cannot run (a playground without CI, an
outage). Commit and push with `om-check-and-commit`, and claim nothing without fresh output
(`verification-before-completion`).

## 4. Independent review in a fresh session

The author never reviews alone. The reviewer has **no shared context**: give it only the diff
range (`<integration>...HEAD`), the spec path and the `om-code-review` skill, and for a later round
the author's answers to the last report (outside Yulius: a subagent or a new session with no
shared context). It is read-only: no edits, commits or label changes. The rules of the review,
the answers to its findings and what may become an issue are `om-code-review`'s.

After an approval, fix the remaining nits within 20 product lines and merge without another round:
`delivery_merge` with the approved head counts them (tests and documents do not count). More than
that, or a file leaving the change, needs a new round.

## 5. Pull request

Small: one concern, about 400 changed lines outside tests and generated files; split larger work
into chained tasks, or, when its slices share one spec and one reviewer's context, into several
pull requests of one task, each slice a step from the start. The body says what changes for whom, links the spec and the issue
(`Fixes #N`), and lists the evidence: commands run and their results, the review verdict and its
head. (In Yulius the pull request is opened for you when your work is ready; its body names no
issue. GitHub closes an issue by `Fixes #N` only for a merge into the repository's default
branch: when the integration branch is the default, add `Fixes #N` with `gh pr edit`;
otherwise (default branch `main`, integration branch `dev`), close the issue yourself after the
merge.)

## 6. Merge on green

A pull request merges when, on its **exact head**: the project's gate passed on our own CI (no
GitHub Actions, no paid rulesets: Yulius CI runs the gate and records it, see `ci_status`), the
independent review approved, the spec is valid when one is required, the steps of what it
delivers are done with none in progress, and the branch is up to date with the integration
branch. A merge delivers the finished part of the plan: steps still ahead stay open, and the
task goes on with them as its next pull request, begun once this one is merged (a commit on
the branch drops an armed merge; a Yulius playground delivers a task once, with its whole
plan). Features are squash-merged into the
integration branch; nobody else will press the button (in Yulius: `delivery_merge`, which
refuses anything that is not green).

Arm the merge as soon as the review approved the head, and never hold a green pull request back
so as not to disturb another one's run. In Yulius a merge queue orders the merges: your pull
request's own run always finishes, it merges the moment it is green and up to date, and when the
integration branch moved since its run, Yulius tests it once on top of the current one (with the
others waiting, as one merge candidate) and merges it on green. `yulius wait` says where yours
stands. Do not arm the same head again, and do not merge the integration branch only because it
moved: that restarts your own run. Merge it for a conflict you are told about.

Yulius tells your session what happens to your open pull request, without a wait running: a red
run on its head or on a merge candidate holding it (the failing jobs and tests, and where to read
the log), a conflict with the integration branch, an armed merge dropped or refused and why, its
merge, why a green head is not merging yet, a check-in when the session has been quiet for a
while, and a pull request that made no progress for an hour. Each message ends with the next step.
When it says "Hands off" (a merge candidate holds your pull request), do not arm again, merge the
integration branch or push: that takes it out of the candidate. Never merge by hand (`gh pr
merge` is refused): the queue merges, and `delivery_status` says why it has not yet.

When a check fails, it is a defect until proven otherwise: reproduce it, fix the cause on the
same branch, test first. Never rerun until green, never skip, never weaken a test or a gate.

## 7. Deploy (CD), separately

With a merge-first staging target (a dev-stage `[cd.<name>]` with `merge_first = true`):
- Your pull request merges on its quick checks.
- Build, deploy and test locally first with the project's `[local]` deployment, which targets only
  your sandbox.
- Once your whole delivery merged (every pull request of the task), ask for staging once (in
  Yulius: `cd_deploy` of that environment).
- After asking, your task is done: do not keep the session waiting for the staging run.
  Yulius records the staging outcome on your delivery. If a finished
  task receives a typed outcome, answer in one line and change nothing. Only a person reopens its work.
- A task still at work repairs a red integration branch before it delivers when nobody else has
  the repair. Yulius names one working task to repair it and tells the other workers who has it.
  Leave an assigned repair to its owner. If nobody is working, project CI and Status offer a
  person a rerun or a new task. Staging verifies the fix and serves the inherited requests.
- Production is promoted only from the revision staging verified.

A merge deploys nothing by itself. CD (deployment) is its own mechanism, like CI: the project's `[cd.dev]`
and `[cd.prod]` (and any other `[cd.<name>]`, of the dev or the production stage) run their deploy
and verify scripts from the protected branches (skill `infra-bootstrap`, step 3). Deploy dev (or
another dev-stage environment) when your task asks for it (in Yulius: `cd_deploy`, then
wait for the run and read it with `cd_status`), or let a nightly automation deploy and test it. A
failed deploy or verification is fixed forward through this same pipeline; when it broke what
dev ran before, first roll dev back to the previous release (in Yulius: `cd_rollback`). Every
environment that deploys has a `rollback` and a `migrations` line (SPEC-415): a change to the
schema or the data keeps the previous release working (expand, then contract a release later)
or makes its deploy print a rollback barrier. Yulius drills the rollback after each dev deploy,
and production deploys wait for the drill of their commit. Production, with its promotion, and
every production-stage environment are a person's decision and action (where the delivery
profile promotes by itself, Yulius promotes after the integration environments; a production
deploy stays a person's): when your task needs one, ask a person (in Yulius: `needs_you`) and
finish the rest.

## 8. Clean up

Delete the remote branch, the local branch and the worktree once merged; leave no stale
branches, worktrees or tags (`git-hygiene`; in Yulius: `repo_hygiene`, `repo_cleanup`). Close
the issue (by the merge into the default branch, or yourself after the merge), and add a line to
the changes log when people using the project will notice the change.

## Test reliability

1. Never assert after a fixed sleep; wait for a condition with a timeout
   (`test-driven-development/condition-based-waiting.md`).
2. Inject the clock and seed randomness (or log the seed).
3. Each test owns its data (unique keys) and cleans up; sort before comparing collections.
4. Locks, transactions and migrations are tested against the real database engine.
5. A skipped test is not a pass; flaky tests follow `test-driven-development`.
6. Show that a regression test fails without the fix.
7. A change of a wire format updates the golden fixtures and both sides.

## What "green" means

| Gate | Rule | Checked by |
| --- | --- | --- |
| Base | the pull request targets the integration branch, is not a draft | Yulius, which opens it |
| Issue | it links exactly one ready issue | you and the review |
| Size | about 400 changed lines outside tests and generated files, or a recorded reason | you and the review |
| Tests | a behavior change changes a test; a bug fix starts with a failing test | you and the review |
| CI | the project gate passed on the exact head, on our own machines | `delivery_merge` |
| Review | an independent approval exists for the exact head, or for one followed by at most 20 product lines, with no later changes requested | `delivery_merge` (`reviewed_head`) |
| Fresh | the branch is up to date with the integration branch | `delivery_merge` |

What we leave out on purpose: human approvals, paid GitHub rulesets and Actions minutes, module
locks (claim paths and read the other task's status instead), and metrics dashboards.

Parallel tasks claim the paths they change (`scope_claim`). On an overlap both tasks are told: wait for the other's pull request to merge and merge the integration branch before editing those files, or agree who changes what by a note on the claim. The other's status (`task_get`: its plan, pull request, claims and screen) says where it stands.
