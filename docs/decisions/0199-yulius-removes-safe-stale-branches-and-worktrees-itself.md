# 0199. Yulius removes safe stale branches and worktrees itself

## Status

Accepted. Specified in [SPEC-269](../../.ai/specs/SPEC-269-i-don-t-understand-the-idea-of-manual-cl.md);
task #255. Amends SPEC-001 DLV-007 (people and agents removed stale items; the scan only listed
them).

## Context

The repository hygiene scan ran every 6 hours and only listed stale branches and worktrees; a
person (the CI tab's Clean up) or an agent (`repo_cleanup`) removed them. The items it calls safe
lose nothing by construction: GitHub keeps a merged or closed pull request's commits ("Restore
branch"), a local branch is only listed when its commits are on GitHub, and a worktree is removed
only when Git finds it clean, never a live task's. A person asked what the manual step was for and
saw nothing happen after it: the list asked for a decision that has one answer.

## Decision

- The background round (10 minutes after start, then every 6 hours) runs the same cleanup a
  person's Clean up runs, for every live sandbox, as `yulius`: a fresh scan, the same checks, each
  removal audited `hygiene.cleanup`. Only safe items go; unmerged work without a pull request
  (`idle`) stays listed for a person, never removed automatically.
- The report says when the round runs next (`next_cleanup_at`) and what the last cleanup did
  (`last_cleanup`: when, who, removed, kept with reasons). It is kept in memory per sandbox; a
  round with nothing to do does not replace it; the audit log is the history.
- Clean up stays, as "do it now".

## Alternatives

- **Explain the button and show its result, keep it manual**: fixes the feedback, not the reason
  for the question; leftovers keep piling up until someone presses it.
- **Clean when a task ends, is archived or its pull request merges**: more triggers to keep correct
  (merges on GitHub outside Yulius have no event here); the bounded round already exists.
- **A per-project switch to keep leftovers**: nobody asked for it and the round removes only what
  loses nothing; one can be added when a project needs it.
- **Persist the last cleanup in the store**: a migration for a status line; the audit log already
  keeps every removal.

## Consequences

- Branches of pull requests merged or closed on GitHub, local copies and finished tasks'
  worktrees disappear within 6 hours without anyone acting; agents' `repo_cleanup` after a merge
  becomes a speed-up, not the only way.
- A closed pull request's branch is deleted too; reopening that pull request needs "Restore
  branch" on GitHub first.
- The round costs one more scan per sandbox only when it removed something.
