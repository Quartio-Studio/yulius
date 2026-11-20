# 0405. A confirmed empty Claude prompt releases a held message

## Status

Accepted for [SPEC-572](../../.ai/specs/SPEC-572-deliver-the-empty-prompt-box-fix-from-th.md). Amends [ADR-0309](0309-yulius-types-nothing-onto-a-persons-possible-draft.md) and retains the restored-draft guard in [ADR-0384](0384-visible-prompt-text-holds-session-messages.md).

## Context

Backspace in an empty conversation and clearing a draft both record a viewer keystroke. Neither produces a submit hook. Issue #1260 traced the resulting permanent hold and attention flag to the shared session typing path. PR #1266 exercised Claude Code 2.1.323 in a PTY and reviewed the attributes of the focused conversation input.

## Decision

After the recent-typing minute, release a keystroke hold only when Claude Code’s alternate screen places the cursor at the first input cell after the prompt marker. Require full-width, undimmed, unnamed rules immediately above and below. Accept a blank hint row or a complete context-usage notice above the upper rule. The input can be blank or contain the renderer’s dim suggestion, including an inverse first glyph when dim text follows it.

Order the observed key and its recovery mark with the existing draft lock and input sequence. Recheck the key before delivery. PR news may retry an exact held-message flag on this evidence. The visible-draft check still applies at paste time, including to a stash restored after a submit.

## Alternatives

Inferring emptiness from Escape, Backspace or Ctrl+U would depend on the current dialog and editing mode. Treating absence of visible draft text as proof of emptiness would also release holds on unrecognized screens.

## Consequences

An empty box can release messages after restart or without a submit hook. A named box, stash hint, dialog, dim agents view, other harness or unfamiliar layout keeps the keystroke hold. A person can still release that hold by submitting their own prompt. The reader uses the existing bounded VT grid and returns only a boolean.
