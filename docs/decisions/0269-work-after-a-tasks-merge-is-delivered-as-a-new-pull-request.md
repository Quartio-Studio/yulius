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
  the merge and the worktree's head carries work the merged pull request did not, Yulius sets the
  record back to `none` (a compare-and-set on the stage, after checking the record still names
  that pull request), audits `delivery.follow_up`, and delivers as for any task: an open pull
  request of the branch is taken on, otherwise it pushes and opens a new one. The readiness rules
  (spec, plan, clean worktree) are checked before the reset, so a head that cannot go out keeps
  the task merged. New work means all three, read from a fresh fetch: the merged head is an
  ancestor of the worktree head; the head is more than it plus Git's own clean merges of the
  integration branch (the review carry-over of SPEC-083 REV-001); and what the head changed
  since the merged head, merged into the integration branch (`merge-tree --merge-base`), changes
  that branch's tree or conflicts (a follow-up a person merged outside Yulius adds nothing, so it
  opens no duplicate).
- **Nothing new keeps the task merged.** The merged head, a head behind it, only clean merges of
  the integration branch and a head the integration branch already holds change nothing. That
  verdict is kept per task for its head and pull request, so later Stops at the same head ask
  neither GitHub nor Git.
- **What Yulius cannot judge is said, not guessed.** A rewritten branch (the merged head no
  longer its ancestor: the answer gives the `git merge` that brings it back), a merged head not
  in the clone (the answer gives the fetch of `pull/N/head`), a failed fetch, a merged pull
  request GitHub does not answer for, or a live merge train holding the task (it counts on its
  members' stages until it ends) change nothing; `delivery_merge` refuses with that reason, and
  the end of a turn delivers nothing and notes it for `yulius wait`.
- **`delivery_merge` never arms on a merged pull request.** With nothing new it answers
  `merged: true`, that the pull request is already merged.

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
- A merged task's Stop at a head not judged yet costs a token lookup and one GitHub read (the
  merged pull request), plus fetches of the clone when the head is past the merged one; open steps do not save it, because `delivery_merge` needs the verdict whatever the plan. A
  head judged covered costs nothing at its later Stops (kept in memory: a restart asks once
  more). An unknown verdict is asked again at the next Stop.
- A task in a live merge train is not delivered anew until the train ends, so the train never
  sees a member's stage go back.
- An armed merge whose pull request merged outside Yulius is still not dropped by the armed-merge
  upkeep (pre-existing, #1075); `delivery_merge` called again clears it.
- `adds_to` needs Git 2.40 (`merge-tree --merge-base`); an older Git makes the verdict unknown,
  never a duplicate pull request.
- The merged pull request leaves the task's record; it stays in GitHub and in the audit log
  (`delivery.follow_up` names it).
