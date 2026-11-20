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
- EBX-003. The one-minute typing window, ten-minute draft flag, restored-draft veto, explicit owner consent and concurrent key ordering continue to apply. After resize, require a live repaint. A reattached host may attest that its captured grid already received that repaint at the restored dimensions. Legacy hosts retain the hold until there is live evidence. A recovered first-key mark needs one quiet minute before the screen can release it.

## Plan

- [x] Reproduce the empty-box hold on current dev with the reviewed PTY fixture
- [x] Recognize Claude Code’s empty box while preserving draft and attention guards
- [x] Verify watch recovery, restart marks and restored drafts
- [ ] Obtain independent approval for delivery to dev

## Acceptance criteria

1. After Backspace in an empty box, a message waits inside the minute and is delivered alone after it. The MCP watch can send its command after eleven minutes without flagging the task or printing a notice.
2. Text left in the box blocks delivery and flags the task after ten minutes. Clearing it releases both MCP and PR news on the next eligible pass. A different attention reason still blocks PR news.
3. Only a matching screen permits release. Stashes, visible restored drafts, a new keystroke during cleanup and ambiguous screens hold their messages. Restart preserves the same outcome.

## Test plan

Run the reviewed PTY scenarios on dev before changing delivery. Exercise the VT screen boundaries, watcher recovery, recovered mark and a key interleaved with delivery. Run SPEC-537 restored-draft and cleanup tests alongside the carried regressions. Check repository contracts, mutation results, independent review, exact-head CI, dev delivery and the GitHub issue state.

## Decisions

Port the reviewed positive evidence of emptiness separately from the broader visible-draft veto. Use current dev's sequence-aware mark lock for the observed keystroke. The host tracks renderer provenance and returns it in a version 2 snapshot. A version 1 snapshot still displays the captured terminal, and only new live output can make it evidence of emptiness. An unfamiliar screen retains the keystroke policy defined in [ADR-0309](../../docs/decisions/0309-yulius-types-nothing-onto-a-persons-possible-draft.md).

## Evidence

Six regression cases failed against dev a7ab520f9 before the typing changes. The PTY showed the empty input. Regression commit 3d5baec04 carries the fixture and failing assertions. Two minutes after Backspace the sender received a draft conflict. At eleven minutes the reconnect pass raised the held-message flag. Ctrl+U left both watches blocked, including without a prompt hook, and an empty reattached screen still held the message.

The ported VT reader and PR #1266 agreed on all 639 locally available capture points from Claude Code 2.1.323. Both matched all 588 entries of the saved verdict list. The 51 additional captures provide a comparison with the reviewed implementation. This was offline replay, and the end-to-end session tests use the fake CLI.

All 41 task-message integration tests passed on the initial port, including both restored stash paths, owner consent, same-millisecond mark cleanup, hookless and restarted sessions, MCP and PR news, a new key and a replacement stash hint during the empty-box lookup.

Round one reviewed 23bc6f222 and found two own-code failures. Shrinking a 40-column screen to 20 columns cropped the stash hint and produced a false empty result. With a two-minute-old first key and Ctrl+U immediately before restart, the recovered sender received success inside the typing minute. Both regression tests failed on the reviewed product. The fix requires the live renderer to clear and repaint the alternate screen after a resize or snapshot, and dates the quiet minute from recovery for a mark whose latest keystroke time is unknown. The earlier first-key time still controls the standing draft warning.

Round two reviewed 839f7e59b. Its 120 focused tests and three PR watch tests passed. Against a loopback model stub, the reviewer found that an unchanged-size SIGWINCH produced 31 bytes and no erase in Claude Code 2.1.323 and 2.1.329. Resizing from 120×40 to 115×40 produced one erase on both versions. The fixture now mirrors the unchanged-size behavior. A second own-code finding showed that a literal J in the same PTY chunk as a real erase suppressed the repaint. Both tests failed on the reviewed implementation. The replacement tracks a complete erase across read boundaries and carries the host's current-grid evidence across attachment. Parked, cropped, legacy and differently sized snapshots cannot attest to an empty prompt.
