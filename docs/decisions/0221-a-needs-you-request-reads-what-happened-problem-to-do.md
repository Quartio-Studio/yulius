# 0221. A Needs you request reads What happened, Problem, To do

## Status

Accepted. Specified in [SPEC-327](../../.ai/specs/SPEC-327-hey-i-really-like-those-notifications-ca.md);
task #311. Extends [0111](0111-agents-ask-a-person-with-needs-you.md).

## Context

"Can we make it a bit more clearly with clear to do for a user […] I propose: what happened,
problem, to do for a user or decision for a user." `needs_you {action, why?, link?}` rendered as
one run of text, "Needs you: <action> · <why>". Agents filled `why` with what happened, the problem
and reassurances in one paragraph, so the to-do was hard to find and a decision read like a chore.

## Decision

- `needs_you` takes `happened` (at most 300 characters), `problem` (500), `action` (200, required)
  and `decision` (false by default). The attention bar shows "Needs you" (or "Needs your decision")
  over a labelled list: What happened, Problem, To do (or Decide), each only when given, the to-do
  last and emphasised; backticked text is code. The sidebar and notifications read
  "Needs you: <action>" or "Needs your decision: <action>" without backticks.
- `why` stays an accepted input, the older name of `problem` (agents started before keep the old
  schema until they restart); both at once is refused. Stored rows with `why` read as `problem`
  (a serde alias), so no migration. The API view drops `why`.
- The tool's answer carries `print`, the request in the same parts, for the agent to print in its
  session.

## Alternatives

- **Keep one `why` and ask agents to format it.** Free text gives no reliable parts to label, and
  the bar could not emphasise the to-do.
- **Parse `why` into parts.** Guesswork on agents' prose.
- **A list of options for a decision.** Buttons would need an answer path back to the agent; a
  decision names its options in `action` and the person answers with Reply, as today.

## Consequences

- A request names what a person must do or decide in a fixed place, the last and strongest line.
- The bar is taller than one paragraph's worth of short lines only when an agent gives all three
  parts; it still floats over the terminal and wraps its labels above their text when narrow.
