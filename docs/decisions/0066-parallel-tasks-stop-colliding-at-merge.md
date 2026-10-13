# 0066. Yulius gives out record numbers, merges the shared logs by union, and keeps reviews across clean integration merges

## Status

Accepted. Specified in
[SPEC-083](../../.ai/specs/SPEC-083-merge-conflicts-with-many-agents.md); extends SPEC-020
SPC-001 and SPEC-050 DLV-203.

## Context

With 8 to 12 agents at once (2026-10-12/13), pull requests failed or were refused at merge for
reasons the parallel work itself created:

- Numbered records (decision records, migrations) took the next free number of each task's own
  checkout. The second to merge failed the repository check, renumbered, needed a new review,
  and often collided again: 81 renames in 36 of 74 sessions, 6 of the 11 failed CI runs retained.
- Every task adds an entry at the top of the changes log and a line to the decision index, as
  Yulius's briefing asks. Git's union driver resolves that locally, but only where the repository
  commits a `.gitattributes`; GitHub's own merge ignores merge drivers (13 armed merges refused
  on 2026-10-12 evening, before Yulius merged the integration branch locally first).
- `delivery_merge` needed a review of the exact head, so every merge of `origin/dev` by the agent
  needed another review round: 33 of 57 merges spent one on dev merges or renumbering.

## Decision

1. **Record numbers are given out by Yulius.** The `record_number` tool returns the next free
   number of a directory's numbered files, past the integration branch (fetched), every live
   task's checkout (uncommitted files included) and every reservation, and reserves it for the
   task in the store (`record_numbers`). Asked again before its file exists, a task gets the same
   number. A task's spec number comes from the same reservations. The briefing tells agents to
   use it for ADRs and migrations.
2. **The shared logs merge by union in every repository.** At every task start Yulius keeps
   `merge=union` for the changes log and the decisions index or file, read from `yulius.toml`, in
   its own block of the clone's `info/attributes`, which all task worktrees read. Yulius already
   merges the integration branch locally before a merge (DLV-203), so GitHub never sees the
   conflict.
3. **A review carries over Git's own merges of the integration branch.** `delivery_merge` accepts
   a reviewed head the branch moved past only through merges whose second parent is on the
   integration branch and whose tree is exactly what Git's merge gives (with the merge drivers).
   The integration branch is fetched first, so a moved remote-tracking ref cannot pass unmerged
   work off as merged. Anything else needs a new review.
4. **Pull requests whose merge is not armed are not updated by Yulius.** Their conflict state on
   GitHub blocks nothing, and every push would start a CI run.

## Alternatives

- **Renumber at merge time, or name records by date or slug.** Renumbering after a review changes
  reviewed files; dates and slugs would fix this repository only, while onboarded repositories
  and migration tools keep numbers.
- **Derive free numbers only from files, without reservations.** A number given out is not in any
  file until the agent writes it; two agents asking within that window would collide.
- **Changelog fragments (one file per change).** No shared file, but every onboarded repository
  would have to change its documents, and the briefing's "recent changes" would read a
  directory.
- **Commit `.gitattributes` in every repository at onboarding.** Possible for new repositories,
  but a repository that drops or never adds the line breaks every armed merge; the logs are
  Yulius's convention, so Yulius keeps them mergeable.
- **Keep every open pull request current (push the integration branch into it on every move).**
  With 8 to 10 open pull requests, the integration branch moving up to 4 times an hour and runs of
  10 to 17 minutes on two lanes, that is 30 to 40 runs an hour, several times what the lanes
  finish; the armed merges would wait behind them, and each push could race the agent's next
  turn.
- **Accept any reviewed ancestor.** A merge with a conflict resolved by hand, or a change slipped
  into a merge, would then land unreviewed.

## Consequences

- Holes in a numbering are possible (a task that reserved a number and was archived without using
  it); duplicates are not, unless someone numbers a file without the tool.
- `info/attributes` belongs partly to Yulius: lines outside its block are kept, its block is
  rewritten at every task start. People working in the sandbox clone by hand get the same union
  merges.
- An agent's own clean dev merge costs only the CI run at the new head; a conflict it resolves
  still needs a review.
- Prose documents every feature edits and code still conflict for real; those stay with the
  agent, and a semantic conflict (a compile error after a dev merge) is caught by CI at the merged
  head.
