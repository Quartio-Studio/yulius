---
name: delivery-pipeline
description: The one autonomous path every feature and fix takes, from a ready issue to verified production - spec, slice-by-slice implementation, independent review in a fresh session, fixes, gates, a small pull request, merge on green, deploy and verify, promotion, cleanup. Use for any change that will be delivered, and whenever you wonder what comes next.
---

# Delivery pipeline

Every feature and fix takes the same path. There is no human acceptance step: the gates are
machine-checked and the review is independent, so you decide, record why, and move on.

```text
ready issue → spec → slices (test first) → local gates → independent review → fixes
  → pull request → merge on green → deploy dev → verify → promotion → verify production → cleanup
```

Research, questions about the product, documentation and small chores skip the spec (and
usually the review); they still follow the Git and gate rules when they change the repository.

## 0. Ready

Work starts from one concern with acceptance criteria. A GitHub issue is **ready** when it has:

- exactly one `type:` label (bug, enhancement, docs, chore, performance), one `priority:` label
  (high, medium, low), at least one `area:` label, and `risk: high` when it touches
  authentication, permissions, data isolation, migrations, money or a shared contract;
- a summary, current and expected behavior, what is out of scope, and a checklist of acceptance
  criteria.

Missing labels are created with the issue, never skipped (in Yulius: `issue_create`). Follow-ups,
bugs you find and flaky tests become their own issues instead of widening the current task.

Before designing, know the product: read the docs index (AGENTS.md, product summary,
architecture, decisions, earlier specs, recent changes), the parallel and previous tasks, and
search earlier sessions for how the area was built. Extend what exists; never build a second
version of it.

## 1. Spec (features and fixes)

Use `om-spec-writing`. The Plan is 3-12 steps of **similar effort**, each a slice that leaves the
project working and is named by what it delivers ("Store drafts per user", not "Implement").
Those steps are the task's progress: 9 of 10 done must mean about 90 %. A fix's spec can be
short: symptom, root cause (`om-root-cause`), the failing test, the fix.

## 2. Slices, test first

Work on your own branch and worktree (`git-hygiene`). For each step: mark it in progress, write
the test that fails without the change and watch it fail (`test-driven-development`), make it
pass with the smallest change, refactor, mark the step done. One step at a time; never tick a
batch at the end. Behavior and refactoring go in separate commits; docs change in the same pull
request as the behavior they describe; risky schema changes are expand → migrate → contract.

## 3. Local gates

Run every gate of the project before anything leaves your machine (`om-check-and-commit`),
and claim nothing without fresh output (`verification-before-completion`).

## 4. Independent review in a fresh session

The author never reviews alone. Start a reviewer with **no shared context**: a subagent (or a
new session) given only the diff range (`<integration>...HEAD`), the spec path and the
`om-code-review` skill. It is read-only: no edits, commits or label changes. It judges the code
and the tests separately, runs the gates, and returns a verdict for the exact head commit.

- Every finding is checked on its merits (is it true here?), then fixed test-first: a test that
  shows the problem, then the fix. Answer each finding in the pull request.
- Re-review after fixes. At most 3 rounds; then decide, record the reasoning in the pull request,
  and continue.

## 5. Pull request

Small: one concern, about 400 changed lines outside tests and generated files; split larger work
into chained tasks. The body says what changes for whom, links the spec and the issue
(`Fixes #N`), and lists the evidence: commands run and their results, the review verdict and its
head. (In Yulius the pull request is opened for you when your work is ready.)

## 6. Merge on green

A pull request merges when, on its **exact head**: the project's gate passed on our own CI (no
GitHub Actions, no paid rulesets: Yulius CI runs the gate and records it, see `ci_status`), the
independent review approved, the spec is valid when one is required, every step is done, and the
branch is up to date with the integration branch. Features are squash-merged into the
integration branch; nobody else will press the button (in Yulius: `delivery_merge`, which
refuses anything that is not green).

When a check fails, it is a defect until proven otherwise: reproduce it, fix the cause on the
same branch, test first. Never rerun until green, never skip, never weaken a test or a gate.

## 7. Deploy, verify, promote

After the merge, the integration environment deploys and its verification runs (smoke tests
against the deployed app; in Yulius: `delivery_deploy`). A failure is fixed forward through the
same pipeline. Then the integration branch is promoted to production (a merge commit, or
fast-forward where the profile says so; `delivery_promote`) and production is verified the same
way. Delivery is done only when production verification passed. `delivery_status` shows where
the change is.

## 8. Clean up

Delete the remote branch, the local branch and the worktree once merged; leave no stale
branches, worktrees or tags (`git-hygiene`; in Yulius: `repo_hygiene`, `repo_cleanup`). Close
the issue through the merge, and add the entry to the changes log.

## Test reliability

1. Never assert after a fixed sleep; wait for a condition with a timeout
   (`test-driven-development/condition-based-waiting.md`).
2. Inject the clock and seed randomness (or log the seed).
3. Each test owns its data (unique keys) and cleans up; sort before comparing collections.
4. Locks, transactions and migrations are tested against the real database engine.
5. No retries: a flaky test is reproduced under load and fixed, or quarantined with an issue
   and an owner. A skipped test is not a pass.
6. Show that a regression test fails without the fix.
7. A change of a wire format updates the golden fixtures and both sides.

## Machine-checked gates (what "green" means)

| Gate | Rule |
| --- | --- |
| Base | the pull request targets the integration branch, is not a draft |
| Issue | it links exactly one ready issue |
| Size | about 400 changed lines outside tests and generated files, or a recorded reason |
| Tests | a behavior change changes a test; a bug fix starts with a failing test |
| CI | the project gate passed on the exact head, on our own machines |
| Review | an independent approval exists for the exact head, with no later changes requested |
| Fresh | the branch is up to date with the integration branch |

What we leave out on purpose: human approvals, paid GitHub rulesets and Actions minutes, module
locks (claim paths and talk to the other task instead), and metrics dashboards.
