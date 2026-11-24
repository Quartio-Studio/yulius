# 0467. A reviewer's report ends its round when its hooks missed it

## Status

Accepted for SPEC-633 (issue #1726). Extends ADR-0400's round lifecycle.

## Context

A Claude Code review round is opened by the Agent tool's `PreToolUse`, linked by `SubagentStart`
and ended by `SubagentStop`. Quartio-v2 task #377's round 2 stayed open after its reviewer
approved and its pull request merged, and `delivery_status` showed it as "no report yet".

Every reviewer Yulius lets start runs as a background agent (all 172 launches in this project's
transcripts on 2026-11-23). It reads for minutes while the author's turn ends and while Yulius may
restart or update. Sessions survive a restart, but a hook sent while the server is down fails open,
so a `SubagentStop` can be lost. A `PreToolUse` that outlasts the hook client's 5 s still inserts
its round in a task of its own, after Claude Code sent `SubagentStart`, so that start links
nothing. In both cases nothing ended the round.

## Decision

- **Stop fallback.** A reviewer's `SubagentStop` whose id no round carries links and ends the
  newest open round that has no subagent and started at or before the stop, while no round of the
  task carries that id.
  - Any reviewer agent's round qualifies. The `PreToolUse` answer that would have named the decided
    reviewer was lost with the start, so the reviewer ran as asked.
  - The round then records the agent and model that ran, and the reason `round` when the agent
    differs, so it no longer claims Opus or `no_fable` for a Fable run.
- **Transcript fallback.** The transcript watch reads a completed background agent's
  `<task-notification>` in the session's own thread as `Observation::SubagentReported`:
  `<status>completed</status>`, `<task-id>` is the agent id, and the report is in `<result>`.
  - It counts only when Claude Code wrote the record as its own notification
    (`origin.kind = "task-notification"`, on the user record or the `queued_command` attachment),
    the text is exactly one notification, and the agent did not stop with work of its own still
    running.
  - The result runs to its last closing result tag, so a report quoting the tags keeps its own
    verdict. Its last 4096 bytes are kept.
  - It ends the open round linked to that agent with the result's verdict, at the record's time,
    also while a reattached run's watcher catches up after a restart.
  - Rounds that started after that time are never ended by it, and a round its stop already ended
    is left alone.
- A round with no report when its pull request merged keeps no end. The delivery view carries
  the merge time (`merged_at`), and `delivery_status` reads such a round as "unreported (the pull
  request merged without its report)".

## Alternatives

- A `PostToolUse` hook for the Agent tool: a background agent's fires at its launch, before any
  report, and it is lost in a restart just as `SubagentStop` is.
- Forward `PreToolUse`'s `tool_use_id` and match it later: `SubagentStart` and `SubagentStop`
  carry no tool use id, so only the transcript could use it, and the stop fallback already
  covers the start that came first.
- Make the review decision faster than 5 s: it shrinks the window but cannot close it under host
  load, and it does not help a lost stop.
- End a merged open round at the merge: an invented end would count it as reported in the review
  numbers.

## Consequences

A reported round ends after a restart once the watcher replays the transcript, and when its start
came first. A round neither hook nor transcript can place still reads unreported after the merge:
- The start came before the round and the stop was lost too: the notification links nothing.
- A usage limit cut the reviewer off.
- The first stop was lost and the reviewer was resumed in the seconds before the reattached
  watcher replayed the first report. The first report then ends the round the resume continues,
  and the second report finds no open round. The review numbers count such
rounds as rounds without a report, as before.
