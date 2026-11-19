# 0382. Session index resumption replays redaction context

## Status

Accepted for [SPEC-535](../../.ai/specs/SPEC-535-preserve-redaction-state-across-session.md), issue #1453.

## Context

Session search stores a byte offset and a line number with each batch of passages. Redacting a tool result can require an earlier file path, a command continuation, or a preceding model response. SQLite can resume at any complete line after a restart.

## Decision

When a resumed batch has a complete record, replay the indexed prefix through the transcript parser on the same open file. Only the new records produce passages. Count the replay against the refresh budget and allow the current batch to finish. A read failure charges the possible replay and new bytes. If the offsets disagree with the file, reset the document’s index with a compare-and-set on the saved byte offset. Clear the derived index once on upgrade so earlier passages are rebuilt under this rule.

## Alternatives

Persisting parser state would avoid replay but require a second durable representation of harness-specific call IDs, continuations and step flags. A fixed lookback would miss calls whose results arrive much later.

## Consequences

A restarted index worker needs only the saved offsets. Files with frequent small appends cost additional reads. Each replay covers at most the indexed prefix of one session, and the refresh budget includes completed replays before moving on to another session. Search results fill back in as upgraded sandboxes refresh.
