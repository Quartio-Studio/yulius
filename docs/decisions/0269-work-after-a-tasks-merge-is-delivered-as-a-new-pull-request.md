# 0269. Work after a task's merge is delivered as a new pull request of the same task

## Status

Accepted. Specified in [SPEC-425](../../.ai/specs/SPEC-425-yulius-doesnt-pick-up-things-this-is-on.md);
task #407 ("yulius doesnt pick up things"), #1074. Extends the automatic delivery of #68 and
SPEC-116 DMG-001.

## Context

A task delivers through one record: its stage and its pull request. Once that pull request is
merged, the record stays on it (#68, so the task reads "Done · merged"), and the automatic
delivery (`try_auto_open_pr`, run at the end of a clean turn and by `delivery_merge`) did nothing
past `pr_open`. Agents do go on working in a task after its merge: a reviewer's late finding, a
failed deploy, a person's next message. Those commits stayed in the worktree, and
`delivery_merge` armed a merge of the merged pull request, which in the standard profile
(checks required, a merged pull request has none) waited forever. On a project in a free private
organisation the agent had to push and open PR #2 by hand, and the task still showed PR #1.

## Decision

- **New work after the merge starts a new delivery of the same task.** When the record is past
  the merge and the worktree's head carries work the merged pull request did not (its head is
  an ancestor of the worktree head, and the head is more than it plus Git's own clean merges of
  the integration branch, read as the review carry-over of SPEC-083 REV-001 reads it), Yulius
  sets the record back to `none` with a compare-and-set, audits `delivery.follow_up`, and delivers
  as for any task: an open pull request of the branch is taken on, otherwise it pushes and opens a
  new one, under the same readiness rules (spec, plan, clean worktree).
- **Nothing new keeps the task merged.** The merged head, a head behind it, only clean merges of
  the integration branch, a merged pull request GitHub cannot answer for, and a rewritten branch
  (the merged head no longer its ancestor) change nothing.
- **`delivery_merge` never arms on a merged pull request.** With nothing new it answers that the
  pull request is already merged.

## Alternatives

- **A new task for the follow-up**, created by Yulius or demanded from the agent. Rejected: the
  session, its context and its branch are this task's; a new task would start in a new worktree
  from `dev` without the commits, and the agent cannot hand its session over.
- **Refuse with "already merged, open a new task".** Rejected: it ends the stuck wait but still
  leaves the work undelivered; the user's complaint is that Yulius does not pick it up.
- **Keep a list of pull requests per task.** Rejected for now: the record holds one, the Git page
  shows one, and the merged one stays on GitHub and in the audit; the current delivery is what a
  person acts on.
- **Treat any head different from the merged one as new.** Rejected: an agent that merges `dev`
  into its branch after the merge (as the briefing asks) would open an empty pull request.

## Consequences

- A task with work after its merge is no longer Done until that work merges; the Git page and the
  task header show the new pull request.
- One GitHub read (the merged pull request) and a fetch run at each clean Stop of a merged task
  whose plan is complete and whose branch changed; tasks with no new commits stop at the cheap
  checks.
- The merged pull request leaves the task's record; it stays in GitHub and in the audit log
  (`delivery.follow_up` names it).
