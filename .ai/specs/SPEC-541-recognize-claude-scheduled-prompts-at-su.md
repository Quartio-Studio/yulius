---
yulius-task: c814981b-f6e8-4963-ada0-f2f7297c9b3f
status: in-progress
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
- [ ] Reproduce draft, goal and question regressions
- [ ] Classify scheduled submissions and verify typed submissions
- [ ] Review and deliver the fix on green CI

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
