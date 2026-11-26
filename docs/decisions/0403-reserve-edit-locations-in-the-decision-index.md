# 0403. Reserve edit locations in the decision index

## Status

Superseded on 2026-11-26 (audit L10 F8): the index is generated from the records by
`python3 scripts/check_repository.py --decision-index`, and the slots, `scripts/decision_slots.py`
and the index's union line in `.gitattributes` are gone. Parallel additions still merge: the
repository check accepts the entries in any order, and the regeneration keeps the committed order,
so the union merge Yulius applies in its clones passes whichever branch is checked out first. One
case the slots did not have remains: two branches that change neighbouring entries (a title or a
Status mark beside a new record) can leave both versions of a line, and the check asks for a
regeneration.

## Context

Parallel task numbers are unique under [decision 0066](0066-parallel-tasks-stop-colliding-at-merge.md).
Their Markdown entries previously shared one insertion point. Git's union merge ordered those
lines according to which head was checked out. Both the manual dev update and the merge train
can check out the higher-numbered head first.

## Decision

Give upcoming numbers hidden Markdown comments. An author inserts the complete link and
description immediately before its numbered comment and keeps that comment in place. The protected
repository check validates the layout using the tree contents, so its result applies to any
commit with that tree. The author's
`scripts/decision_slots.py` verifies that each marker used after this migration exists at
the branch's merge base with dev. It replenishes the empty reserve in blocks.
A reservation that exhausts the reserve needs a shared empty extension before use.

## Alternatives

An external merge driver needs registration in every clone and execution in confined host Git.
Sorting the index during CI changes the checkout after the candidate tree has been recorded.
A fixed slot for every four-digit number would add thousands of unused lines.

## Consequences

Git merges independent additions using the ordinary index and tree. Editors can find a slot by
number. Unused slots have no visible content on GitHub. Two different entries assigned to one
number fail the repository check. [SPEC-568](../../.ai/specs/SPEC-568-keep-the-adr-index-valid-when-parallel-y.md)
