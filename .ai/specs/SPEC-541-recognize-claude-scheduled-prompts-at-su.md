---
yulius-task: c814981b-f6e8-4963-ada0-f2f7297c9b3f
status: done
---

# SPEC-541: Recognize Claude-scheduled prompts at submission

## Goal

Preserve a person's unsent draft, achieved goal and unanswered request when Claude Code wakes itself with a scheduled prompt. Fixes #1225.

## Context

SPEC-468 CCF-010 protects the draft until a prompt from the box goes through. SPEC-471 established that scheduled work gets a plain-text submit hook after dequeue. The current text predicates accept that hook as typed. The draft test covers a tagged notification and the scheduled fake-agent test covers queue accounting without a draft.

## Requirements

- CSP-001 A scheduled hook preserves the draft, including its restart veto.
- CSP-002 Scheduled work continues the working stretch, preserves an achieved goal and leaves an open request for a person to answer.
- CSP-003 Ordinary submissions, including identical text subsequently typed by a person, retain their existing behavior. Classification survives hook retries.

## Plan

- [x] Measure scheduled and typed submissions with Claude Code
- [x] Reproduce draft, goal and question regressions
- [x] Classify scheduled submissions and verify typed submissions
- [x] Review and deliver the fix on green CI
- [x] Verify Chromium completes the interrupted navigation before teardown

## Acceptance criteria

- AC1 A test-clock submit and a fake-agent scheduled hand-over both preserve an unsent draft.
- AC2 With recent typing, an achieved goal and an open needs_you survive the scheduled submit. A later typed submit retains the ordinary effects.
- AC3 Record the real hook's distinguishing evidence and timing. Update SPEC-468 CCF-010 and the operations guide.

## Test plan

Compare the hook inputs and transcript records in an isolated Claude Code PTY with a synthetic Messages API. Reproduce through the internal listener on the test clock and through the fake agent in a PTY. Exercise a same-text typed control. Run scoped tests, quick checks, mutation checks and independent review.

## Decisions

The local CLI showed that the current user record cannot identify the submit before the hook answers. Use the scheduled fire and its queue hand-over, and the user record to identify spent fires. See [ADR-0388](../../docs/decisions/0388-claude-scheduled-submits-use-the-transcript-hand-over.md).

## Evidence

Claude Code 2.1.328 ran in a PTY against a loopback Messages API stub with its own home and config. The typed submit and the scheduled submit carried the same seven keys. The scheduled fire was timestamped at 21:07:00.544 UTC, its enqueue at .546 and dequeue at .571. All three records were readable during the scheduled hook. No user record with its prompt ID appeared during the one-second hold. At Stop the user record had that prompt ID, the fire UUID as `scheduledFireId`, `isMeta: true`, and `promptSource: "system"`. The retained 2.1.322 synthetic run has the same ID links. Its queue writes could still be buffered at hook entry.

On the baseline, both new tests in `cargo test -p yulius --test main scheduled_` failed at the draft assertion. Two additional harness tests reproduced a lost fire outside the bounded read and a background notification still in the queue. An unidentified hand-over preserves the personal state and keeps ordinary scope screening.

Round 1 reviewed `f28bc0e884045023ee84f3d6f846e622a95ceec3`. New harness tests failed on that head for an old process's stranded enqueue, a display-only loop name and a clipped fire preceding a visible enqueue. All seven classifier tests passed with the corrections. The authenticated attempt's start bounds replay of queue operations. Full enqueue text identifies expanded loops and text containing a Unicode joiner. A visible matching dequeue with incomplete history keeps the draft and open request. SPEC-471 CCQ-004 now describes the scheduled hook's restart guard.

Round 2 reviewed `2aab3a87db3fab4b5c24207bfa8b7ab496da99f4`. With a stranded notification, the second tick of a repeating schedule was misread as typed. The two-tick test failed on that head and passed after reconciling the queue with the completed fire ID and accounting for the stranded notice on each hand-over. Eight classifier tests passed, including a same-text typed submit after the ticks.

At `3ec660448cb9e393cbeec7590df7b0e10186c718`, the delivered-notification control failed. A dequeue identifies neither the notice nor the scheduled item behind it. The classifier now keeps both candidates until the user record identifies the entry. All nine transcript tests passed, including a delivered notice followed by a scheduled hand-over and two recurring hand-overs behind a stranded notice. Replaying the captured 2.1.328 transcript through its pre-response queue records returned `Scheduled`.

Independent review approved `3c5ba4295cc9945a6e4f0217e6f81745bf409915` after the delivered-notification correction. Both red integration regressions passed when run separately: the test-clock draft and open question, and the fake-agent scheduled turn with a draft and achieved goal. `scripts/dev quick` and spec validation passed. `scripts/dev security` reported that Vidoc is disabled for this project.

At the approved head, `cargo test -p yulius-harness --lib scheduled::` passed all nine classifier tests and `cargo test -p yulius --test main scheduled_ -- --test-threads=1` passed all five selected integration tests. Nextest ran all 58 tests matching `/^claude_code_moves::moves::/` in isolated processes and all passed. Concurrent execution in one libtest process crossed existing race callbacks registered under the shared test task ID. This was also reproduced on the pre-fix test binary, and recorded on #1225.

The whole-branch mutation run reported `TIMEOUT Unmutated baseline in 1018s build + 300s test` and tested no mutants. The first review-fix run stopped on the unchanged migration-helper signal-order test before it could test any of its 13 mutants.

The second review-fix mutation run stopped at its unmutated baseline in the existing instance-migration estimate test. It tested none of the 17 listed mutants. The scoped Clippy run at the approved head passed for the app, harness and fake agent with warnings denied.

The scheduled integration tests (5/5) and classifier tests (9/9) also passed after the clean merge of dev at `e90a4a743`. The final review-fix mutation run passed its unmutated baseline (3,131 tests). Its summary was `mutants: 33 listed, 15 caught, 6 missed, 0 timed out, 2 unviable, 10 not reached (bound 30 min, took 1802 s)`. Five missed mutations let an unrelated typed hook lose its ordinary effects. Strengthened controls require the exact origin. The sixth retired the next queue item on an ordinary prompt record. A control with two ordinary prompts ahead of a schedule now exercises that case. Replaying each of the six cargo-mutants diffs against the strengthened tests failed. The unchanged classifier passed all ten tests. Ten mutations remain untested within the campaign’s 30-minute bound.

CI run `716424e1-a005-4fa4-82ef-83b8d2a6f2cb` failed in the existing initial-load browser test. The trace showed the second intentionally aborted navigation committing Chromium’s error document during the teardown error check. The failure reproduced in four of eight runs at the approved head. The test now waits for that specific main-frame navigation while still requiring both connection resets. All 16 focused repetitions and all five tests in `tests/initial-load.e2e.ts` passed with retries disabled.
