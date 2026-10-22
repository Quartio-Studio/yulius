# 0199. Yulius removes safe stale branches and worktrees itself

## Status

Accepted. Specified in [SPEC-269](../../.ai/specs/SPEC-269-i-don-t-understand-the-idea-of-manual-cl.md);
task #255. Amends SPEC-001 DLV-007 (people and agents removed stale items; the scan only listed
them).

## Context

The repository hygiene scan ran every 6 hours and only listed stale branches and worktrees; a
person (the CI tab's Clean up) or an agent (`repo_cleanup`) removed them. The items it calls safe
are meant to lose nothing: GitHub keeps a merged or closed pull request's commits ("Restore
branch"), a local branch is only listed when its commits are on GitHub, and a worktree is removed
only when Git finds it clean. An unattended round needs those guarantees to hold at the moment of
each deletion, which the scan alone did not give (review round 1 of SPEC-269). A person asked what the manual step was for and
saw nothing happen after it: the list asked for a decision that has one answer.

## Decision

- The background round (10 minutes after start, then every 6 hours) runs the same cleanup a
  person's Clean up runs, for every live sandbox, as `yulius`: a fresh scan, the same checks, each
  removal audited `hygiene.cleanup`. Only safe items go; unmerged work without a pull request
  (`idle`) stays listed for a person, never removed automatically. One cleanup of a sandbox runs at
  a time.
- The checks hold at the deletion, for the round and a person alike:
  - a task's worktree is in use unless its task rests (closed, stopped or failed): a queued or
    paused task keeps it; the task is read again right before the removal;
  - a task's worktree goes only when Git ignores nothing in it but rebuildable caches (the disk
    upkeep's rule, SPEC-019): an `.env` or a local database keeps it;
  - a branch of a merged or closed pull request is safe only while its tip is the pull request's
    head: commits pushed after it closed make it unmerged work;
  - a local copy is judged again against GitHub after the cleanup's remote deletions and a fresh
    fetch; when the fetch fails, local copies stay.
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
