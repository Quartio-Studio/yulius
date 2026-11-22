# 0451. Repaint an unreadable Codex terminal before typing

## Status

Accepted for [SPEC-613](../../.ai/specs/SPEC-613-an-idle-codex-session-takes-messages-aft.md).

## Context

The model picker can be cropped or partway through a redraw. After reattachment, an idle child can leave the terminal in that state indefinitely.

## Decision

Use a temporary PTY size change to request a repaint from the attached child. Restore the original dimensions, including when the host reconnects or the server restarts midway through the probe, and judge the current screen before sending input. An unreadable screen after the bounded probe raises a Needs you on the task. Keep an owed PR event pending until submission succeeds. Before the first temporary resize, atomically record the intended width and height in the run directory. Later viewer resizes update that record. A retrying automatic prompt releases its own request once the terminal is readable. A private message stays with its sender to resend. Its Needs you stays open when the screen becomes readable.

## Alternatives

A prompt placeholder can survive cropping. Waiting for unsolicited terminal output can leave an idle task waiting indefinitely. Restarting the child interrupts the conversation and discards its terminal state.

## Consequences

A probe can briefly redraw a viewer's terminal. An unresponsive child gives the owner a concrete recovery action. Existing hosts can receive the resize through their current protocol.
