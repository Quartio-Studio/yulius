# 0388. Claude scheduled submits use the transcript hand-over

## Status

Accepted for [SPEC-541](../../.ai/specs/SPEC-541-recognize-claude-scheduled-prompts-at-su.md).

## Context

Claude Code gives a scheduled prompt the same submit hook as text a person entered. In the local CLI probe the queue had handed on the scheduled text before that hook. The later system-origin user record carried both the submit ID and the UUID of the fire.

## Decision

Classify the submit once for all its person-facing effects. Use the authenticated task's native transcript. Correlate the fire with the full text Claude enqueued and its hand-over. A matching user record can identify a replay and retires its fire for later submits. Preserve the turn boundary and the queue's restart guard.

## Alternatives

A Stop hook lists schedules that could fire. People can enter exactly the same text while one exists. Waiting for the ID-matched user record before answering holds up the action that writes it. Reading the path supplied by a hook gives provider input authority over a host file read.

## Consequences

The lookup has a bounded file read and allows time for Claude's buffered writes. Older hooks without correlation fields retain the existing text rule. No raw transcript text goes into logs or the database.
