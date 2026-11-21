# 0421. Codex right-click opens the viewer’s clipboard menu

## Status

Accepted for [SPEC-582](../../.ai/specs/SPEC-582-fix-clipboard-on-codex.md). Refines [ADR-0079](0079-the-terminal-mouse-is-the-programs.md).

## Context

Codex CLI 0.170.0 requests mouse tracking. A plain right-button press tells its Linux TUI to read the desktop clipboard. The viewer’s clipboard belongs to the browser on a different machine.

## Decision

The browser terminal uses the harness in the terminal state message to reserve an unmodified right-click for its clipboard menu when Codex has the mouse. It intercepts the press before xterm sends a mouse report. Choosing Paste reads the viewer’s clipboard and sends the text through xterm’s existing paste path.

## Alternatives

Disabling Codex’s native right-click paste would suppress the error but leave the gesture with no way to paste. Reading the browser clipboard immediately on right-click would insert text when the person only wanted to see the menu.

## Consequences

People can use the same Paste menu with or without Codex mouse tracking. If the browser refuses clipboard access, the menu shows the existing keyboard shortcut hint. Other programs keep the mouse behavior specified in ADR-0079.
