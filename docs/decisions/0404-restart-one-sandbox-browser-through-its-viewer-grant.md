# 0404. Restart one sandbox browser through its viewer grant

## Status

Accepted for [SPEC-570](../../.ai/specs/SPEC-570-restart-the-locked-browser-of-one-sandbo.md).

## Context

An agent's input before the host policy arrived can leave a native chooser on a sandbox display. ADR-0318 locks viewer input for the lifetime of that browser. A person watching the panel also keeps the idle reaper from stopping it.

## Decision

Use the browser's sandbox write grant for a restart. Hold the sandbox's process slot through reaping the old instance and bringing up its replacement on the same profile. Start on a blank page, install the existing address guard, then reopen the visible web addresses and select the previously active tab. Retain those addresses in the slot across a failed launch until recovery or sandbox shutdown.

## Alternatives

Clearing the chooser mark on a live display would allow input while a native dialog could still be open. A server restart interrupts other sandboxes. Chrome's native session restore starts loading pages before Yulius can install its request interception.

## Consequences

The initiating panel reconnects to the replacement. Other viewers of this sandbox can reconnect through their existing Retry action. Cookies and persistent site storage stay in the profile. Reopened pages have new document state, tab IDs and navigation histories. Viewer input on a virtual display still requires a successful policy probe.
