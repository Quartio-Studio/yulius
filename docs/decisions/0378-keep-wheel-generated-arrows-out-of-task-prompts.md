# 0378. Keep wheel-generated arrows out of task prompts

## Status

Accepted for [SPEC-529](../../.ai/specs/SPEC-529-fix-this-this-is-a-fictional-bug.md).

## Context

When a full-screen program has mouse reporting off, xterm can turn a wheel gesture into a
cursor key. A task can interpret that key as recall from prompt history. Yulius also records
it as possible draft input.

## Decision

In the task view that offers session history, disable xterm’s wheel handler when the alternate
buffer is active and the program has mouse tracking off. Read the terminal modes on each event.

## Alternatives

Marking the generated arrow as scroll input would still let the program recall text into its
prompt. Ignoring arrow keys in draft detection would also affect real keyboard input.

## Consequences

The task program receives no arrow from this wheel fallback. A viewer uses the existing history offer to read earlier
messages in this state. Native mouse tracking, ordinary scrollback and the shell keep their
own wheel handling.
