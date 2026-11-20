---
yulius-task: b2be5ab4-1158-4661-9318-f82c27114efd
status: done
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
- [x] Obtain independent approval for delivery to dev
- [x] Verify mixed-version PTY recovery and publication guard mutations

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

The final review approved 40a34182b. The reviewer replayed 161 PTY chunk sizes and six real stash repaint intervals and verified both directions of protocol compatibility. A golden frame test on 277c04214 still expected version 1 when encoding the version 2 greeting. The corrected test pins the two wire versions independently. All 54 contract tests and all 168 focused application, domain, host and wire tests passed at 40a34182b. Clippy passed on yulius-app, yulius-domain, yulius-contracts, yulius-ptyhost and fake-agent with all targets and features.

The initial repository mutation run at 23bc6f222 reported 85 listed, 75 caught, 4 missed, 0 timed out, 6 unviable and 0 not reached. The existing exact-head repair-publication test caught all three negated PR watch guards in a focused rerun, including the two the initial integration filter missed. A single dim glyph and dim whitespace after an inverse cursor glyph distinguish a suggestion from a possible draft. Both reader mutations survived on the ported tests. The new assertion caught the missing nonblank condition. Sharing the range after the cursor also removed a duplicate boundary whose mutation was equivalent. All 39 mutations of the resulting standalone reader were caught, all 639 real capture verdicts stayed identical, and all 168 focused repository tests passed again at c02a62782.

The repository run for the first review fixes reported 36 listed, 16 caught, 3 missed, 0 timed out, 17 unviable and 0 not reached. Two survivors affected the chunk-scanning routine replaced after round two, and the third affected the removed SIGWINCH request. The new PTY partition test covers the replaced routine. A further regression against the shared tracker catches changing its alternate-screen-and-erase requirement to an OR. A primary-screen erase leaves a retained, cropped alternate screen unconfirmed.

Dev advanced to 4b3497496 with the parked-Codex change. Reapplying only the regression fixture and tests to that commit reproduced all six original failures again. The task branch merged it cleanly and passed 169 relevant tests, including the new primary-screen rejection. The repository mutation run of the corrected reader caught all seven changed-line mutants. A missing test for the host-confirmed dimensions was added to the shared welcome type. Its two comparison mutations were caught in a targeted rerun. The maximal v1 and v2 snapshot test also checks the allocation for one encoded frame and caught the mutation that reserved eight payloads.

The v2 repository mutation campaign on the corrected golden head reported 56 listed, 37 caught, 3 missed, 0 timed out, 16 unviable and 0 not reached. Focused reruns caught all three surviving variants with the restored-size, maximum-frame and primary-screen regressions. The subsequent range campaign reported 7 listed and 7 caught. The restored-size change campaign reported 8 listed, 4 caught, 0 missed, 0 timed out, 4 unviable and 0 not reached. An unviable mutant failed to compile.

Independent review approved the final product and test head b7b6215178cdf98121c4b2508385ff8436ac2dd3 after the clean integration merge. The reviewer ran all four coverage follow-ups and checked the saved capture verdicts. All 170 focused nextest cases passed at that head, as did targeted Clippy with warnings denied, repository quick checks and spec validation. The Vidoc command reported the scan disabled in Yulius. Interactive renderer probes used a loopback model stub, and the complete message-delivery path used the real PTY host with a fake CLI. Hosted CI and the landed commit are recorded in this task’s delivery evidence.
