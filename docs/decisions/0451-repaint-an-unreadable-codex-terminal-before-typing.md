# 0451. Repaint an unreadable Codex terminal before typing

## Status

Accepted for [SPEC-613](../../.ai/specs/SPEC-613-an-idle-codex-session-takes-messages-aft.md), finished by [SPEC-623](../../.ai/specs/SPEC-623-finish-597-on-claude-code-an-idle-codex.md).

## Context

The model picker can be cropped or partway through a redraw. After reattachment, an idle child can leave the terminal in that state indefinitely.

## Decision

Use a temporary PTY size change to request a repaint from the attached child. Restore the original dimensions, including when the host reconnects or the server restarts midway through the probe, and judge the current screen before sending input. An unreadable screen after the bounded probe raises a Needs you on the task. Keep an owed PR event pending until submission succeeds. Only the writer refusing it twice raises it; a person's draft or recent key holds it silently, as SPEC-480 says. Before the first temporary resize, atomically record the intended width and height in the run directory. Later viewer resizes update that record; a viewer resize whose record cannot be saved still applies and drops the record, so a reattachment keeps the host size. An automatic prompt (the wake after an outage) raises its own action, which its next send releases once the terminal is readable, matched by its redacted text. A private message stays with its sender to resend. Its Needs you stays open when the screen becomes readable.

## Alternatives

A prompt placeholder can survive cropping. Waiting for unsolicited terminal output can leave an idle task waiting indefinitely. Restarting the child interrupts the conversation and discards its terminal state.

## Consequences

A probe can briefly redraw a viewer's terminal. An unresponsive child gives the owner a concrete recovery action. Existing hosts can receive the resize through their current protocol.
