---
yulius-task: a87fb46a-b4fc-4ec0-aa5c-fa3f2c6cb224
status: in-progress
---

# SPEC-537 — Preserve restored drafts when typing into a session

## Goal

A session keeps a draft that Claude Code restored after a submit or a local command.
A sender learns why their message waits. A newer keystroke keeps its recovery mark.

## Context

Issue #1264's Claude Code 2.1.323 PTY run r22 recorded a single UserPromptSubmit
containing the restored draft followed by the message. The submit hook advanced
`box_emptied_ms` and no keyboard input followed the restore. Once the typing window
expired, `type_into_session` allowed the paste. The existing fake CLI never restored
a stash. In `hooks.rs` the draft check and the removal of `person-typed` can also
straddle the next key. PR #1266 separately releases false keystroke holds on proof of
an empty box. Its screen reader has yet to land on dev.

## Requirements

- RSD-001. At the shared session typing entry point, visible draft text in the Claude
  conversation input prevents a paste. The sender receives a conflict with a recovery
  action. A session's person is told when an automatic message waits.
- RSD-002. Preserve delivery to an empty prompt, including its dim suggestion, and
  the explicit task-owner consent and recent-typing rules of SPEC-487 and SPEC-480.
- RSD-003. Draft-mark removal and marking a new key preserve that key across server
  recovery, including when they overlap.

## Plan

- [x] Specify the restored-draft veto and mark lifetime
- [x] Reproduce both stash restores and the mark cleanup race
- [x] Protect visible drafts and preserve concurrent keystrokes
- [ ] Verify the fix and obtain independent approval
- [ ] Prepare the approved fix for delivery to dev

## Acceptance criteria

1. A fake CLI restores a stash after an ordinary prompt and after a local command.
   A refused message leaves both the draft and the submitted prompt intact and tells
   its sender or the session's person why.
2. An empty conversation box takes the message alone.
3. A key arriving during cleanup leaves its mark on disk.

## Test plan

Drive the fake CLI through the PTY with a shifted clock. Check the rendered restored
text, the message response, and the separate prompts after recovery. Exercise the
mark interleaving with a deterministic race point. Run the relevant integration and
screen tests, repository quick checks, security gate, independent review and Yulius CI.

## Decisions

Use positive screen evidence as an additional veto. An unrecognized renderer keeps
the existing keystroke policy because the task supports multiple harnesses. Screen
text stays in the hub and never appears in the refusal or audit. A task owner who
explicitly sends with `with_draft` retains the documented consent path. Each submit
records the last viewer input it observed, including across held-hook retries. This
orders a key and a submit even when they share a wall-clock millisecond.

## Evidence

- The test-first commit adds a fake CLI with a prompt buffer and Ctrl+S stash. On the
  preceding delivery logic, the ordinary-submit case read the draft and incoming
  message as one prompt. The local-command case already held the message through
  the key record. The mark race failed with “cleanup erased the newer key's recovery
  mark” when the new viewer key started between the check and removal.
- The three integration regressions passed with the fix. They check that the sender
  gets a conflict, an automatic reconnect flags the session, and the person's draft
  and the later message each reach the agent separately. The overlapping-key test
  checks the run folder after the older submit and the new key both complete.
- Four VT reader tests passed. Replaying the saved real Claude Code 2.1.323 screen
  bytes through the reader detected the r22 ordinary-submit restore and the r19
  local-command restore. The q6 empty named prompt permitted delivery.
- Repository quick checks passed. Green CI on the reviewed head is required for
  the dev merge.

- The first review found a synchronous test awaiting run setup and a same-millisecond
  ordering case. The run setup test now awaits in a Tokio test. The fixed-clock PTY
  regression failed on the previous implementation with “the older submit erased the
  same-ms key's mark”.
