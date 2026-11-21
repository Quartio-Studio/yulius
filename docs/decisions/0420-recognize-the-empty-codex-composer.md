# 0420. Recognize the empty Codex composer before releasing a held message

## Status

Accepted for [SPEC-581](../../.ai/specs/SPEC-581-i-keep-getting-this-even-is-nothing-is-t.md). Extends [ADR-0405](0405-a-confirmed-empty-claude-prompt-releases-a-held-message.md) to Codex.

## Context

The shared session typing path remembers a viewer keystroke until a prompt is submitted or the renderer confirms an empty input. Codex 0.170.0 displays a dim placeholder after Backspace in an empty box or Ctrl+U at the end of a draft. The Claude screen reader cannot recognize this picture.

## Decision

Use a harness-specific reader for the focused Codex conversation input. Require a visible cursor in the first input cell, a bold standard or Ultra prompt marker, and the complete dim placeholder. A recalled image can leave the editable text empty. Reject its label above the input and require the styled navigation hint in the footer, which the native composer displays only when both text and attachments are empty. A full repaint at the current geometry can establish this evidence. Codex uses an erase to the end of the display with its cursor at home during a resize. The shared tracker recognizes this form of a full alternate-screen erase.

## Alternatives

Taking Backspace or Ctrl+U as proof could discard protection for surviving text elsewhere in the input. Using elapsed time alone could submit someone's unfinished draft with a waiting message.

## Consequences

The existing key ordering, quiet minute, recovery mark and attention settlement govern Codex delivery. An unfamiliar composer or an incomplete repaint retains the hold. A renderer update that changes the placeholder needs new PTY evidence before this reader can accept it.
