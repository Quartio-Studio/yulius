---
yulius-task: fc7e0234-8cbd-4d7a-9b9e-c238f11eeed8
status: in-progress
---

# SPEC-633 A reviewer that reported ends its round; a round its merge left open reads unreported

## Goal

A review round whose reviewer reported (approve or request changes) shows its end and verdict on
the delivery, also when the hooks that link and end it missed it. A round that truly never
reported reads as unreported once its pull request merged, never as still waiting for a report
(issue #1726).

## Context

Quartio-v2 task #377 (id 86103dc0-be80-44f2-a579-9ca6f6d1fea9, Claude Code, Opus 5.5) lists
review round 2 (yulius-reviewer on claude-fable-5-2, head 9077a21e, started 2026-11-23 10:16:39
UTC) with no `ended_at`, `reported: false` and no verdict, although its notes say round 2 approved
and PR #1623 of quartio-v2 merged after it.

A Claude Code round is kept by three hooks (`crates/yulius-app/src/review.rs`, SPEC-493
REV-002 to REV-004): the Agent tool's `PreToolUse` inserts it with no subagent
(`start_review_round`, `started_at` taken at the insert), `SubagentStart` links the newest
unlinked round of that agent started at or before its own send time
(`start_review_subagent`), and `SubagentStop` ends the round carrying that subagent id
(`end_review_round`). Nothing else ends a round; a merge only stamps `merged_at` on the rounds
(`crates/yulius-store/src/delivery.rs`), and `delivery_status` then shows the open round as
"no report yet" (`crates/yulius-app/src/mcp_text.rs`).

### What could be read

#377's hook records and transcript belong to quartio-v2's sandbox, which a task of this project
cannot read (only this project's sandbox is mounted, and `sessions_search` searches this
project). The round data above is the person's own report, made to task #615. The failing path
is therefore named from this project's real records and the code, and each candidate is
reproduced by a test that replays its hook sequence:

- This project's 219 reviewer launches in its Claude Code transcripts (`$HOME/.claude/projects`,
  read for structure only, 2026-11-23): every one that Yulius let start (172) ran in the
  background (`toolUseResult.status: "async_launched"`, with `agentId`), whether or not the agent
  asked for `run_in_background`. The main turn can therefore end, and the instance can restart or
  update, while a reviewer still reads. Its report then reaches the main transcript as a
  `<task-notification>` naming the agent id as `<task-id>`, with `<status>completed</status>`
  and the report in `<result>` (task #615's round 2 reported that way, at 12:16:08 and, resumed
  with `SendMessage`, again at 12:39:36 under the same id).
- Sessions survive a restart or update of Yulius (operations, "restart with sessions"), but a hook
  sent while the server is down fails open (`crates/yulius-harness/src/hook_client.rs`): the
  client gives up and prints nothing, and `SubagentStop` is not one of the held events. Its
  report is lost to the round, while the transcript keeps it.
- The hook client waits 5 s for `PreToolUse` (`HOOK_TIMEOUT`). Its handler runs in a task of its
  own (`crates/yulius-server/src/routes/internal.rs`), so a decision that takes longer (up to two
  move decisions of up to 3 s each, `MOVE_DECISION`, and a `git log`) still inserts the round,
  after Claude Code started the reviewer and sent `SubagentStart`. That start found no round to
  link, and the round, inserted later with a later `started_at`, is never linked: its
  `SubagentStop` ends nothing.

### Failing paths

1. **Lost stop.** The reviewer reported while Yulius did not take its `SubagentStop` (a restart
   or an update of the instance, a hook that failed). Nothing reads the report from anywhere
   else. With #377's round 2 running for up to 49 minutes in the background on the day `main`
   was promoted (08:24 UTC) and installed by the instance later, this is the likeliest path; it
   cannot be confirmed without #377's records.
2. **Start before the round.** `PreToolUse` outlasted the hook client, `SubagentStart` came
   before the insert, and the round stayed unlinked.
3. **Display.** Either way the merged round read "no report yet", as if its reviewer still ran.

Checked and ruled out: a reviewer resumed with `SendMessage` sends `SubagentStart` again with the
same `agent_id` (measured with the real CLI 2.1.325 in task #472, and seen in task #615's
transcript), so a different id at the stop is not a path. A stop after the merge is not one
either: the merge needs the approval of the reviewed head first, which is the stop itself.

## Requirements

- RRE-001: A reviewer's `SubagentStop` whose id no open round carries links and ends the newest
  open round of the same reviewer agent that has no subagent and started at or before the stop,
  with the report's verdict.
- RRE-002: A `<task-notification>` in the session's own transcript that says a background agent
  completed and carries its `<result>` ends the open round linked to that agent id, with the
  result's verdict, at the record's time, unless the round's `SubagentStop` already ended it. A
  replayed notification ends nothing that started after it, so a reviewer resumed after its
  report keeps its continuation open.
- RRE-003: A round that has no report when its pull request merged reads "unreported (the pull
  request merged without its report)" in `delivery_status`; the delivery's round view carries
  the round's `merged_at`. A cut-off round keeps "cut off by a usage limit".

## Plan

- [x] Spec with the failing paths of #377's round 2 named
- [ ] Regression tests replay the three failing hook sequences
- [ ] A reviewer's stop ends the round its start never linked
- [ ] A reviewer's completed notification ends its round when its stop was lost
- [ ] A round its pull request merged without a report reads unreported
- [ ] Decision, architecture and changes entry
- [ ] Independent review approved

## Acceptance criteria

- AC-1 (RRE-001): `SubagentStart` sent before the `PreToolUse` insert, then `SubagentStop` with
  "Verdict: approve", gives the round `ended_at`, verdict `approve` and the subagent id.
- AC-2 (RRE-002): a round linked by `SubagentStart` whose `SubagentStop` never arrives ends with
  the verdict of the reviewer's completed notification in the transcript; a failed or interim
  notification ends nothing; a replayed first report does not end a resumed reviewer's
  continuation.
- AC-3 (RRE-003): an open round of a merged pull request reads unreported in `delivery_status`,
  a round of an open pull request still reads "no report yet".
- Each test fails before its fix.

## Test plan

- `crates/yulius/tests/review_rounds.rs`: AC-1 and AC-3 through the hooks endpoint and the
  delivery text; AC-2 through the transcript observation the watcher sends.
- `crates/yulius-harness`: the notification parser gives the report for a completed agent block,
  nothing for a failed or resumable one (real record shapes from task #615's transcript).
- `crates/yulius-app/src/mcp_text.rs` unit test for the unreported line.

## Decisions

- End from the transcript rather than add a `PostToolUse` hook for the Agent tool: a background
  agent's `PostToolUse` fires at its launch, before any report, and a hook sent while Yulius
  restarts is lost the same way `SubagentStop` is. The transcript is replayed after a restart.
- Link at the stop rather than make `PreToolUse` faster or forward its `tool_use_id`: the stop
  fallback covers every way the start can miss (a slow decision, a lost start) with one rule, and
  `SubagentStart` carries no tool use id to match.
- Keep a merged open round as it is and only read it as unreported: closing it with an invented
  end would count it as reported in the review numbers, which already count it as a round
  without a report.
- ADR-0467 records the first two.
