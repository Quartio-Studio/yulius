# 0384. Visible prompt text holds session messages

## Status

Accepted for [SPEC-537](../../.ai/specs/SPEC-537-preserve-restored-drafts-when-typing-int.md).
Extends ADR-0309.

## Context

Claude Code can restore a stashed draft after a prompt or a local command. Its
UserPromptSubmit hook reports the submitted prompt. The restore produces no new
keystroke, so the recorded submit can outlive the last key while the box holds text.

## Decision

At message delivery, inspect the existing VT screen for the focused input between
Claude Code's full-width conversation rules. Text at normal intensity indicates a
draft. An empty box can display a dim suggestion with its first glyph inverted.
Keep visible drafts for the person to send or clear. Tell a sender in the response
and flag the session for an automatic message. Keep explicit owner consent through
`with_draft` and the existing recent-keystroke guard.

## Alternatives

Tracking Ctrl+S would couple Yulius to one shortcut and miss other text restored by
the CLI. Requiring proof of an empty Claude screen on every delivery would strand
messages on other harnesses and renderer layouts.

## Consequences

The screen can add a hold after a submit. Unrecognized layouts continue to use the
keystroke policy. The hub returns a boolean so a refusal or task flag never copies
the person's text. The recovery mark keeps its existing on-disk format.
