# 0046. A full session is compacted, then restarted fresh, then handed to a person

## Status

Accepted. Specified in [SPEC-060](../../.ai/specs/SPEC-060-fix-213-a-session-whose-context-is-full.md)
(issue #213). It complements the run setting `autoCompactEnabled: true`, which makes a full context
rarer but not impossible.

## Context

When a Claude Code session's context is full, the API refuses every prompt. Claude Code records an
API-error line, "Prompt is too long", and runs no Stop hook. Yulius read that record as nothing, so
the task stayed `working`. On production, task #8 sat like that for three hours with its pull
request unmerged, until a person typed `/compact`. A usage limit is recovered by resuming the same
session past it (ADR-0027), but a resumed full session is still full.

## Decision

The harness observes the refusal, a compaction boundary and a failed compaction. The supervisor
ends the turn (the task waits) and runs one recovery episode per task, kept in the task's runtime
across runs:

1. Type `/compact <focus>` into the live session, with a one-line focus built from the task: number,
   title, goal, branch, pull request and steps left. On the compaction boundary, type one line that
   says the context was compacted and the agent should carry on: the compacted session is idle, and
   the prompts the full context refused are lost.
2. If the compaction fails (the CLI's error, another refusal after it, or no boundary within 10
   minutes), continue the task in a fresh session on the same subscription through the switch
   path, opened with a handoff: goal, branch, pull request, steps left and how to pick up.
3. If the fresh session's prompt is refused too before any successful reply, or the fresh session
   cannot start, set the task to `needs_attention` and say why in the terminal and the audit log,
   once. Type nothing more.

The episode ends at the agent's next successful reply, when a person changes the session in the
TUI (`/clear`, `/resume`), or when the compaction is cut short (its run ended), so a context that
fills again later is compacted again. A restart does not repeat a recovery that already gave up.

## Alternatives

- **Only enable auto-compaction** (done earlier). It does not cover a compaction that gives up,
  and nothing would notice.
- **Restart fresh at once.** Simpler, but it throws away the conversation that `/compact` keeps.
  The person in #213 recovered the session with `/compact`.
- **Retry compaction several times.** A session whose briefing alone overflows would loop. Two
  automatic tries and then a person keeps the cost bounded.
- **Store the attention reason in a column.** The terminal notice and the audit entry already carry
  it, as for a goal that was never taken. A column would add a migration for a flag a person clears
  by acting.

## Consequences

- A task never stays `working` after a refused prompt. Its terminal says what Yulius does.
- Yulius types into a live session in two new places: the `/compact` and the wake-up line. Each
  is typed once, only into the attempt that reported the full context, and only while that attempt
  is live, and only while the episode still asks for it. A person who typed into the session
  since the refusal owns it: Yulius types nothing and says so.
- The fresh session loses the conversation. The handoff points the agent at the repository, the
  steps and the delivery, where the work lives.
- Other API errors that end a turn without a Stop hook (an image the API refuses) still leave the
  task `working` until the next prompt. That is out of scope here.
