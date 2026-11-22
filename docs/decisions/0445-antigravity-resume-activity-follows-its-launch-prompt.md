# 0445. Antigravity resume activity follows its launch prompt

## Status

Accepted for [SPEC-604](../../.ai/specs/SPEC-604-an-idle-antigravity-session-under-the-ge.md).

## Context

After a host move, Yulius queues lost sessions and reopens their recorded conversations. Antigravity opens at its prompt when the launch specifies only a conversation ID. It can display earlier tool calls and Gemini's announcement there. This launch generates no new turn for a Stop hook to settle.

## Decision

Initialize a bare Antigravity resume with the saved idle activity, or Waiting by default. A launch that submits a prompt or continues an interrupted turn starts Working. Native hooks and new transcript observations own subsequent activity changes.

## Alternatives

Parsing the announcement or tool summary would couple turn state to presentation and conversation history. Sending Escape after reading an idle screen could interrupt a turn that started between the read and the keypress. CLI 1.5.12 documents dismissal on an empty prompt and hides the card when a message is sent.

## Consequences

Existing parking and close checks can release an idle Antigravity session. Background work, queued delivery, and new input still pass through their existing guards.
