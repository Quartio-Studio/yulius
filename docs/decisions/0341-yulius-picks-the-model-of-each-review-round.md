# 0341. Yulius picks the model of each review round

## Status

Accepted (2026-11-15). Specified in
[SPEC-493](../../.ai/specs/SPEC-493-faster-from-start-to-merge-opus-writes-f.md) (REV-001 to
REV-004, REV-007, REV-008; the move of REV-007 since fix 1d); task #472. Follows
[ADR-0340](0340-tasks-default-to-opus-5-5-and-fable-is-a-choice.md) (Opus 5.5 writes).

## Context

The person decided (2026-11-13) that rounds 1 and 2 of a pull request's independent review run
on Fable 5.2 at high effort, whatever the task's model, and later rounds on Opus 5.5. In a blind
replay of four real changes a Fable 5.2 reviewer found 8 of 8 blocking defects and an Opus 5.5
reviewer 6 of 8. Until now the reviewer was "a read-only subagent": any agent type, on the
session's model, with nothing that saw which model reviewed. The Agent tool's `model` takes
aliases only, and an alias moved before (an older Claude Code read `fable` as Fable 5.1).

Measured with Claude Code 2.1.325 against a local stand-in for the model API:

- an agent definition's `model` and `effort` are what its subagent's requests carry, whatever
  the session runs;
- a definition given at the launch (`--agents`) outranks one of the same name in the agent's
  home; one in the repository's `.claude/agents` outranks both;
- a `PreToolUse` hook on the Agent tool can answer `updatedInput` with another `subagent_type`,
  and that definition runs;
- `SubagentStart` and `SubagentStop` name the subagent, and `SubagentStop` carries its report;
  a usage limit that cuts a subagent off sends no `SubagentStop`.

## Decision

- **Two reviewer agents, defined at every task launch.** `yulius-reviewer` (`claude-fable-5-2`)
  and `yulius-reviewer-opus` (`claude-opus-5-5`), effort `high`, by full model id, with the
  reviewer's contract as their prompt (read-only, own code or pre-existing, a last line
  `Verdict: approve` or `Verdict: request changes`). Not a file in the agent's home, which it
  can rewrite.
- **The agent always asks for `yulius-reviewer`; Yulius decides the round.** The Agent tool's
  `PreToolUse` reaches Yulius, which numbers the round among those of the task's open pull
  request and answers with the reviewer it runs as: rounds 1 and 2 `yulius-reviewer`, from round
  3 `yulius-reviewer-opus` (`updatedInput`, the rest of the input as asked). The agent is told
  the round and its model in a line of context. A round counts once it ended with a report, so a
  reviewer started again after a cut-off keeps its round's number and reviewer.
- **A session whose subscription cannot run Fable now moves first** (SPEC-493 fix 1d) to the
  best other subscription that can run both its model and Fable: the Agent call is refused with
  why and the session continues there, its turn interrupted first as a person's switch is, with
  a prompt asking for the review again. Holding the hook instead would need every session
  launched again (the hook client never holds `PreToolUse`). Background work or a queued
  prompt, which a move would end, refuses the call with that reason and moves nothing. The move
  is said only once the new session's start there was checked, so the agent is never told to
  wait for a move that is then refused.
- **With no such subscription the round gets the Opus reviewer** (reason `no_fable`): its prompt
  and the agent are told to say so in the review's summary: a reviewer refused at its first
  request helps nobody, and Yulius never waits for Fable.
- **Each round is a record** (`review_rounds`, schema version 62): its number, the head, the
  reviewer and model, why (`round`, `kept` for a reviewer started again for a round with no
  report, `no_fable`), the subagent, when it started and ended, its verdict. The next round
  follows the last one that reported, so a round two of its reviewers reported counts once. The store
  closes a pull request's rounds in the transaction that records its merge.
- **A failure to decide lets the reviewer start as asked** (Fable): the stronger reviewer is the
  safe default.
- **One sentence everywhere the review is described**: "Start the independent code review with
  the Agent tool and the `yulius-reviewer` agent: Yulius picks the round's model (Fable 5.2 for a
  pull request's first two rounds, then Opus 5.5)." The briefing, the definition of done, the
  workflows and `delivery_merge`'s description, which have size budgets, say its short form. A
  test holds every place to it. Since 2026-11-26 (audit L10 F2) the instruction texts no longer
  repeat it: the briefing names the reviewer.

## Alternatives

- **A rule that tells the agent which model to ask for per round.** The agent would count
  rounds, an alias would be the model, and nothing would see whether it was followed.
- **Agent files in `~/.claude/agents`.** The agent can rewrite them.
- **Refuse `delivery_merge` without a recorded approving round.** A session started before the
  update has no reviewer agents, and a real approval can lack the verdict line. Left for when
  the records show how often that happens (SPEC-493 Decisions).
- **Count every start as a round.** A reviewer started again after a cut-off would push round 2
  onto Opus, against the decision that a resumed round keeps its model.

## Consequences

- Reviews of rounds 1 and 2 use Fable credits on the task's subscription; authors no longer do
  (ADR-0340).
- A repository that defines its own `yulius-reviewer` overrides the model on purpose.
- Sessions started before the update have no reviewer agents until their next launch; their
  reviews run as before.
- The rounds are readable afterwards. Since SPEC-493 fix 1c a round also records the model that
  answered its reviewer (read from the subagent's transcript, not from what was asked) and a
  cut-off; a merge writes `delivery.review.models`; `GET /api/analytics/reviews` and the
  Pipeline tab's Review card give the numbers the person reads a week later.
