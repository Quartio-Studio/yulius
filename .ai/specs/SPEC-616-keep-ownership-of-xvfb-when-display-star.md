---
yulius-task: 8fc3f6f7-5e0a-4b3d-8599-8f5c0d378751
status: in-progress
---

# SPEC-616 Keep ownership of Xvfb when display startup and cleanup both fail

## Goal

A sandbox's X server whose start failed and whose stop failed too stays owned by the sandbox's browser slot. The failed stop is reported. No other display starts for the sandbox until that server has been stopped. Fixes [#1519](https://github.com/Quartio-Studio/yulius/issues/1519).

## Context

`VirtualDisplay::start_xvfb` (`crates/yulius-browser/src/display.rs`) spawns Xvfb, waits until it accepts connections and starts the X controller (XTEST input). When readiness or the controller fails, it calls `process.terminate(quick_stop())`, discards the result, removes the cookie and returns the startup error. The `Supervised` handle is dropped at once. The browser manager (`manager.rs`, `BrowserManager::start`) never sees the process, because the display's error returns before the slot records anything. If Xvfb survived SIGKILL, it keeps running unowned, and the next `ensure` or `restart` starts another X server for the same sandbox. Independent review of SPEC-570 (#1410) found this from source. The double failure was never reproduced.

The browser slot already keeps a display or browser whose stop failed. `BrowserSlot::stop` returns before it records `Stopped`, so the next `ensure`, `restart`, idle reap or shutdown retries the stop before a replacement launches. The `tests/manager.rs` test `replacement_cleanup_holds_the_slot_and_saved_tabs_until_the_display_stops` shows this for the fake display. A display that failed before it existed has no such holder.

## Requirements

- XS-001 When Xvfb readiness or X controller setup fails and stopping the server then fails, `start_xvfb` keeps the process handle. The display start fails with a stranded server: its process, display number, runtime directory and cookie, and the startup cause.
- XS-002 The error surfaced to the caller names the startup cause and the failed stop: `BrowserError::DisplayNotStopped { display, pid, cause }`. It is an internal failure for viewers and tools, like `NotReaped`.
- XS-003 The browser slot holds a stranded server as its state. `BrowserSlot::stop` (every `ensure`, `restart`, idle reap and shutdown) stops it again first and fails with the same error while the server survives. No replacement display or browser starts for the sandbox until the stop succeeds.
- XS-004 The cookie and runtime directory stay while the stranded server runs and are removed once it has been reaped.
- XS-005 Once the stranded server has stopped, the next start proceeds normally.

## Plan

- [ ] Keep a failed display start's surviving Xvfb and report its failed stop
- [ ] Strand the Xvfb the same way when X controller setup fails
- [ ] Retry the stranded Xvfb's stop in the browser slot before any replacement display
- [ ] Document the stranded display in the architecture and changes
- [ ] Answer the independent review of the delivery

## Acceptance criteria

- With an injected readiness failure and an injected failed stop, `ensure` fails with `DisplayNotStopped`. The first Xvfb is still running, and a second `ensure` starts no other X server (one recorded Xvfb start).
- The same holds for a failure in X controller setup (an Xvfb without XTEST).
- Once the stop is allowed to succeed, the next `ensure` stops the stranded server, removes its cookie and starts a new Xvfb. With a real Xvfb, the display start succeeds.
- Each of these tests fails on dev without the change.

## Test plan

A real process cannot be made to survive SIGKILL without root (a process in uninterruptible sleep). A test-only seam (`display::stop_faults`, compiled only in this crate's unit tests) makes the display's stop report "not reaped" without signalling, keyed by the display's runtime directory. The Xvfb really keeps running, as in the real double failure. A fake Xvfb script records each start. It can sleep without listening (readiness timeout), exec the real Xvfb with `-tst` (no XTEST, so controller setup fails), or exec the real Xvfb unchanged (success). Parts that need the real Xvfb skip with a note where it is missing, as the existing display tests do.

- Display unit tests: both failure points, a stranded handle whose retry fails while held and succeeds once released. Cookie kept, then removed. Process alive, then gone. A fresh start succeeds.
- Manager unit test: `ensure` while stuck twice, with one Xvfb start recorded. After release `ensure` stops the stranded server and starts a new Xvfb.
- Run `yulius-browser` tests in full, the `yulius` wiring tests that map `BrowserError`, and `scripts/dev quick`.

## Decisions

- The stranded server travels in the display start's error (`DisplayStartFailure`) to the manager. It does not live in a registry inside the display module. The slot already owns every other leftover, retries its stop in each path and blocks replacements. `start_xvfb` cannot block a later start by itself, because it keeps no state between calls.
- `start_xvfb` still tries one quick stop itself, so a failed start leaves no process behind in the ordinary case. Only a failed stop hands the process on.
- The error is a new variant, not `Display(String)`. An unreaped process is an internal failure, and the variant names the pid and display.
- Fault injection uses a test-only seam at the display's stop. A cfg(test) field on `DisplayConfig` would break its `Clone`/`PartialEq`. A flag inside `Supervised` would change every process.
- A cancelled start (the caller's future dropped mid-start) still drops the process handle, whose drop sends SIGKILL. That case is outside #1519.
