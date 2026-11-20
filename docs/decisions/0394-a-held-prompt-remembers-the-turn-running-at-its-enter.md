# 0394. A held prompt remembers the turn running at its Enter

## Status

Accepted for SPEC-555, issue #1206.

## Context

Claude Code submits its prompt hook when the person presses Enter. An earlier turn can reach
its Stop while the new hook waits for capacity. A server update can also occur between the hooks.

## Decision

Record whether a prompt hold began during a running turn in the existing capacity pause row.
The earlier turn can finish while the pause remains open. A prompt that actually reaches the
session appears in its transcript and ends the hold. A capacity release or an abandoned hook
continues to use the existing resume loop.

## Alternatives

The transcript queue entry appears after the prompt hook returns. The age of an open pause
cannot identify which turn sent the Stop. A mark kept only in the server process is lost on
restart while the agent and its held hook can survive.

## Consequences

The additive SQLite migration preserves pre-upgrade open prompt holds until a transcript
record, restored capacity or abandonment settles them. The public pause kind stays compatible.
A prompt held from idle retains its established recovery when its CLI runs that turn after
giving up the hook.
