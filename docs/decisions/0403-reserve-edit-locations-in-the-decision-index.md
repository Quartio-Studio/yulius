# 0403. Reserve edit locations in the decision index

## Status

Accepted for Yulius's own repository.

## Context

Parallel task numbers are unique under [decision 0066](0066-parallel-tasks-stop-colliding-at-merge.md).
Their Markdown entries previously shared one insertion point. Git's union merge ordered those
lines according to which head was checked out. Both the manual dev update and the merge train
can check out the higher-numbered head first.

## Decision

Give upcoming numbers hidden Markdown comments. An author inserts the complete link and
description immediately before its numbered comment and keeps that comment in place. The repository check verifies the
layout and keeps a block of slots available ahead of the newest decision.

## Alternatives

An external merge driver needs registration in every clone and execution in confined host Git.
Sorting the index during CI changes the checkout after the candidate tree has been recorded.
A fixed slot for every four-digit number would add thousands of unused lines.

## Consequences

Git merges independent additions using the ordinary index and tree. Editors can find a slot by
number. Unused slots have no visible content on GitHub. Two different entries assigned to one
number fail the repository check. [SPEC-568](../../.ai/specs/SPEC-568-keep-the-adr-index-valid-when-parallel-y.md)
