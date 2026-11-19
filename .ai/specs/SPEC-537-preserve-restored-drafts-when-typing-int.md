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
- [ ] Reproduce both stash restores and the mark cleanup race
- [ ] Protect visible drafts and preserve concurrent keystrokes
- [ ] Verify the fix and obtain independent approval
- [ ] Deliver the approved fix to dev

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
explicitly sends with `with_draft` retains the documented consent path.
