---
yulius-task: b2be5ab4-1158-4661-9318-f82c27114efd
status: in-progress
---

# SPEC-572 — Release messages when Claude Code shows an empty prompt box

## Goal

A key that leaves the conversation input empty delays a session message for one minute. A person can clear a draft and receive a waiting message without submitting another prompt.

Fixes #1260

## Context

Issue #1260 reported a permanent hold and a misleading Needs your action flag after Backspace. PR #1266 at 18bfbe364 contains a reviewed screen reader, PTY regressions and real Claude Code 2.1.323 observations. It conflicts with dev. At a7ab520f9 the shared typing path checks the recorded keystroke before the visible-draft veto and the PR watch skips a flagged session. [SPEC-537](SPEC-537-preserve-restored-drafts-when-typing-int.md) also orders concurrent draft marks and protects restored stashes.

## Requirements

- EBX-001. After the recent-typing window, the shared typing path can release a possible draft only on the reviewed full-screen Claude conversation picture. A partial frame, a stash hint, an ambiguous view, a withheld screen or another harness provides no such proof.
- EBX-002. A person's message reaches the agent as a separate prompt. An automatic message gets through on the next eligible watch pass. The empty box leaves no held-message mark, terminal notice or attention flag. Other attention reasons and requests for a person still block PR news.
- EBX-003. The one-minute typing window, ten-minute draft flag, restored-draft veto, explicit owner consent and concurrent key ordering continue to apply. After reattachment, judge the recovered mark against the recovered screen.

## Plan

- [x] Reproduce the empty-box hold on current dev with the reviewed PTY fixture
- [ ] Recognize Claude Code’s empty box while preserving draft and attention guards
- [ ] Verify watch recovery, restart marks and restored drafts
- [ ] Obtain independent approval for delivery to dev

## Acceptance criteria

1. After Backspace in an empty box, a message waits inside the minute and is delivered alone after it. The MCP watch can send its command after eleven minutes without flagging the task or printing a notice.
2. Text left in the box blocks delivery and flags the task after ten minutes. Clearing it releases both MCP and PR news on the next eligible pass. A different attention reason still blocks PR news.
3. Only a matching screen permits release. Stashes, visible restored drafts, a new keystroke during cleanup and ambiguous screens hold their messages. Restart preserves the same outcome.

## Test plan

Run the reviewed PTY scenarios on dev before changing delivery. Exercise the VT screen boundaries, watcher recovery, recovered mark and a key interleaved with delivery. Run SPEC-537 restored-draft and cleanup tests alongside the carried regressions. Check repository contracts, mutation results, independent review, exact-head CI, dev delivery and the GitHub issue state.

## Decisions

Port the reviewed positive evidence of emptiness separately from the broader visible-draft veto. Use current dev's sequence-aware mark lock for the observed keystroke. An unfamiliar screen retains the keystroke policy defined in [ADR-0309](../../docs/decisions/0309-yulius-types-nothing-onto-a-persons-possible-draft.md).

## Evidence

Six regression cases failed against dev a7ab520f9 before the typing changes. The PTY showed the empty input. Two minutes after Backspace the sender received a draft conflict. At eleven minutes the reconnect pass raised the held-message flag. Ctrl+U left both watches blocked, including without a prompt hook, and an empty reattached screen still held the message.
