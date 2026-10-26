# 0225. Claude Code owns the goal retry after an API error; Yulius follows it

## Status

Accepted. Specified in [SPEC-333](../../.ai/specs/SPEC-333-this-api-error-should-trigger-retry-and.md);
task #317, issue #927. Refines [ADR-0212](0212-a-turn-whose-stop-yulius-never-answered-settles-at-its-record.md)
for the turns Claude Code retries.

## Context

An API error ("API Error: Connection lost mid-response") ends a turn with no Stop hook. ADR-0212
settles such a turn at the record that closes it, so the task read "Your turn" (or "in review"
with a pull request). With a goal active, Claude Code 2.1.292 retries the turn itself. It writes
`Goal still active · <cause> · retrying in <n> min (<k>/3) · send a message to retry now` and
delivers a "Goal check-in" prompt 1, 5 and 15 minutes later (+0–20 %). After three failed retries
it writes `Goal paused after 3 automatic retries · <cause> · send a message to continue` and
stops. An error it does not retry pauses the goal at once (`Goal paused · the API rejected the last
request`, `· the last turn could not finish`, `· the request was rate limited`). Every Yulius task
runs under `/goal`. Task #33's person saw "in review" while the retry was armed and typed
`continue` themselves.

## Decision

- The harness reads these notices as `GoalRetry { armed, notice }`. Pauses that other paths own
  are not read as `GoalRetry`: a hook's stop (Yulius's own hooks), a goal check's timeout and a
  usage limit (SPEC-072).
- While a retry is armed, the record that closes the turn leaves the task Working. A task that
  read Waiting goes back to Working. The retry's prompt, a person's prompt, agent work or an
  interrupt ends the armed state, and so does a new run.
- Spent retries, or a pause on an API error, put the task in Needs you. Its activity is
  NeedsAttention, and Yulius opens a request when none is open: What happened (Claude Code's
  notice), Problem (nothing goes on by itself), To do (send the task a message). A typed prompt
  ends it, as every request ends.
- The replay after a restart folds the last notice (`TranscriptFacts::goal_retry`) and applies
  the same rules.
- Yulius types nothing itself.

## Alternatives

- Yulius retries by typing `continue` after its own delay: a second prompt would race Claude
  Code's check-in, and the two retry counts would disagree. Claude Code also drops its own
  check-in when a message is queued. Rejected while every task runs under `/goal`.
- Keep the error turn Waiting and only rename the state: the task would still wait for a person
  who has nothing to do.
- Read the error record itself (`isApiErrorMessage`, `error: server_error`) instead of the notice.
  Whether Claude Code retries depends on its classification (transient or not, the goal state).
  Only its notice says what it decided.

## Consequences

- A goal turn that an API error ends reads Working through Claude Code's retries (up to about 21
  minutes), and Done when a retry achieves the goal.
- Spent retries show in the sidebar's Needs you group with their cause. The audit records
  `task.goal_retries_spent`.
- The match follows Claude Code 2.1.292's wording. A CLI that rewords the notices falls back to
  ADR-0212 (Your turn), never to a task stuck Working.
- A turn without a goal (none is active) still settles as Your turn after an API error: Claude
  Code does not retry it.
