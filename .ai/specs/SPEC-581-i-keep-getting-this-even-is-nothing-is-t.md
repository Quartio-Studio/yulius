---
yulius-task: 053eac70-0fdb-46ba-8e1f-c23a5c21bf23
status: in-progress
---

# SPEC-581 — Deliver waiting messages to an empty Codex prompt

## Goal

A person who presses Backspace in an empty Codex terminal, or clears a draft there, can receive waiting PR news without submitting an extra prompt.

Fixes #1583.

## Context

Task #565 shows a held-message warning alongside terminal output. The screenshot omits the input line and the preceding gesture. In a PTY running the pinned Codex CLI 0.170.0, Backspace and clearing text with Ctrl+U leave the focused composer showing a dim “Ask Codex to do anything”. The shared message path consults screen evidence exclusively for Claude Code. The older tests exercise Claude's input and scroll gestures. [SPEC-572](SPEC-572-deliver-the-empty-prompt-box-fix-from-th.md)

## Requirements

- CDP-001. A waiting message reaches a recognized empty Codex composer after one quiet minute. A successful delivery settles its held-message flag and recovery mark.
- CDP-002. Text, attachments, dialogs, recent typing and unrecognized pictures keep their existing protection. The shared delivery path rechecks the key and screen before pasting.
- CDP-003. After a resize, accept emptiness only from a grid repainted at its current size. Codex repaints by erasing from the home position to the end of the display.

## Plan

- [x] Reproduce the empty-box warning and specify the correction
- [x] Prove the false hold and protect genuine drafts with regression tests
- [ ] Correct the draft signal and verify queued delivery
- [ ] Protect recalled images and recognize the Ultra prompt
- [ ] Obtain independent approval for delivery to dev

## Acceptance criteria

1. An automatic message held for a Codex draft reaches the session as its own prompt after the person clears the draft and the quiet minute expires. The warning clears.
2. A Codex placeholder is recognized with its observed attributes and cursor. Typed lookalike text, an image token, a hidden or displaced cursor and unfamiliar screens still hold delivery.
3. Resizing cannot turn a cropped draft into evidence of emptiness. A full Codex repaint can reestablish evidence even when the PTY splits its erase sequence across reads.

## Test plan

Probe the pinned CLI with synthetic content. Replay its composer through the rendered grid and exercise the shared PR watch with a fake agent and a shifted clock. Check the resize tracker with full and partial erases. Run focused Rust tests, repository checks, mutation testing, independent review and Yulius CI.

## Decisions

Use the existing one-minute and draft-mark rules with positive evidence from the pinned Codex renderer. A timeout cannot establish what is in the input. Record only synthetic screen examples in the repository.
