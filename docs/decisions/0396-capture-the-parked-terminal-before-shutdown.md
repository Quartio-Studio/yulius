# 0396. Capture the parked terminal before shutdown

## Status

Accepted for SPEC-558.

## Context

A CLI can erase its terminal while handling shutdown. Codex leaves token usage and resume instructions on the screen. The owner already resumes a task through Yulius.

## Decision

Capture the redacted terminal in the attempt directory at the parking decision, with the observed idle duration and the instruction to write to resume. Rename the pending snapshot to its stable filename after the task’s Parked transition commits. A queued resume then keeps the same visible screen, including when a concurrent switch or an escalated loss ended the attempt. A cold reader can recover a pending snapshot from that committed transition if the server restarted before the rename. The terminal hub marks the capture as immutable until a new host snapshot takes ownership. The WebSocket state carries this marker so a smaller owner viewport can fit the same grid throughout a resumed launch. Use the regular final screen for other outcomes. Refuse a park when the snapshot cannot be saved.

## Alternatives

Filtering shutdown text cannot recover a cleared TUI. Changing the process termination signal couples the display to each CLI's shutdown behavior. Injecting a new user message after Stop introduces a race with owner input and other wakeups.

## Consequences

A parked run keeps an additional bounded terminal snapshot. Older parked attempts can show only the content retained by their final snapshot.
