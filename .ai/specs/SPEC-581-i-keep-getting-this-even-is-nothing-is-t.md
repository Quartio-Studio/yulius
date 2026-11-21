---
yulius-task: 053eac70-0fdb-46ba-8e1f-c23a5c21bf23
status: done
---

# SPEC-581 — Deliver waiting messages to an empty Codex prompt

## Goal

A person who presses Backspace in an empty Codex terminal, or clears a draft there, can receive waiting PR news without submitting an extra prompt.

Fixes #1583.

The merge-train relay tests keep their TCP port reserved through startup. Fixes #1572.

## Context

Task #565 shows a held-message warning alongside terminal output. The screenshot omits the input line and the preceding gesture. In a PTY running the pinned Codex CLI 0.170.0, Backspace and clearing text with Ctrl+U leave the focused composer showing a dim “Ask Codex to do anything”. The shared message path consults screen evidence exclusively for Claude Code. The older tests exercise Claude's input and scroll gestures. [SPEC-572](SPEC-572-deliver-the-empty-prompt-box-fix-from-th.md)

## Requirements

- CDP-001. A waiting message reaches a recognized empty Codex composer after one quiet minute. A successful delivery settles its held-message flag and recovery mark.
- CDP-002. Text, attachments, dialogs, recent typing and unrecognized pictures keep their existing protection. The shared delivery path rechecks the key and screen before pasting.
- CDP-003. After a resize, accept emptiness only from a grid repainted at its current size. Codex repaints by erasing from the home position to the end of the display.

## Plan

- [x] Reproduce the empty-box warning and specify the correction
- [x] Prove the false hold and protect genuine drafts with regression tests
- [x] Correct the draft signal and verify queued delivery
- [x] Protect recalled images and recognize the Ultra prompt
- [x] Obtain independent approval for delivery to dev
- [x] Keep the relay fixture port reserved throughout merge-train testing

## Acceptance criteria

1. An automatic message held for a Codex draft reaches the session as its own prompt after the person clears the draft and the quiet minute expires. The warning clears.
2. A Codex placeholder is recognized with its standard or Ultra marker, observed attributes, cursor and native empty-composer footer. Typed lookalike text, an image token, a recalled image with empty text, a hidden or displaced cursor and unfamiliar screens still hold delivery.
3. Resizing cannot turn a cropped draft into evidence of emptiness. A full Codex repaint can reestablish evidence even when the PTY splits its erase sequence across reads.
4. A competing listener cannot take the test forward's port during setup. The relay restart, drop count and link tests pass together.

## Test plan

Probe the pinned CLI with synthetic content. Replay its composer through the rendered grid and exercise the shared PR watch with a fake agent and a shifted clock. Check the resize tracker with full and partial erases. Run focused Rust tests, repository checks, mutation testing, independent review and Yulius CI.

## Decisions

Use the existing one-minute and draft-mark rules with positive evidence from the pinned Codex renderer. A timeout cannot establish what is in the input. Record only synthetic screen examples in the repository.

## Evidence

The three new regressions failed on dev `6a566d9db` before the correction. The PR watch raised Needs your action with a draft, then returned no notice after Ctrl+U and a quiet minute. The screen test rejected the empty placeholder and the PTY tracker rejected the resize repaint.

Independent review of `74a1c5c37` found two own-code issues. The reader accepted a placeholder with a recalled image and rejected an Ultra prompt. Both failed against that head in VT replay. Codex 0.170.0 draws recalled image labels above the text area and derives its navigation footer from both text and attachments. [Native composer](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/tui/src/bottom_pane/chat_composer.rs#L1195-L1197) · [Footer hints](https://github.com/openai/codex/blob/rust-v0.170.0/codex-rs/tui/src/bottom_pane/chat_composer/passive_hints.rs#L316-L329)

With synthetic credentials and a loopback model endpoint, Codex 0.170.0 displayed both its standard and Ultra empty markers. Typing the placeholder produced plain draft text. Pasting a PNG produced an image token. After clearing each, the native UserPromptSubmit hook received only the synthetic PR notice. The live resize capture replayed through the bounded VT reader at 90×28.

At `112a82697`, 73 focused nextest cases passed across task messages, the terminal hub, Claude and Codex input readers, restored drafts and the resize tracker. The PR-watch test observed the notice as its own prompt, removal of the recovery mark, settlement of the warning and zero repeat deliveries. Targeted Clippy with all targets and features and warnings denied passed. Independent round two approved this head and separately ran the Codex composer, resize and PR-watch regressions. `scripts/dev quick` passed. `scripts/dev security` reported Vidoc disabled for the project.

`scripts/dev mutants` caught all 31 changed-line mutants on the approved head. The run finished in 611 seconds with no misses, timeouts or untested mutants. The recalled-image case was verified against the pinned renderer and terminal replay. The live CLI image check used a locally pasted PNG.

The PR head passed Yulius CI. Merge-train run `235dc68c-1bf0-4277-a98a-6449c7a7ab12` failed in the sandbox relay restart test with `AddrInUse`. The fixture dropped its listener and rebound the port after starting its server. A competing bind in that interval reproduced the error. Issue #1572 records the same gap in another caller of the limited forward.

At `222ea422c` the restart regression failed because a competing listener claimed the port before server startup. At `1c3c9d56f` all three limited-forward fixtures pass their original bound listener into the forward. All 62 relay tests passed together, including the restart, drop count and link tests. The updated branch passed 79 terminal and message tests, sandbox Clippy with warnings denied and the repository quick checks.

`scripts/dev mutants --base 222ea422c` passed its baseline and finished in 270 seconds. All eight generated replacements for returning a Tokio join handle failed compilation. The run recorded zero caught, missed, timed out or unreached and eight unviable. The competing-bind regression is the behavioral evidence for this fix.
