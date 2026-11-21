---
yulius-task: 2bfb2cd1-d319-4db4-bbb7-e1467fffa1a0
status: in-progress
---

# SPEC-593. The sandbox browser starts on a moved host: stale Chrome profile locks are cleared

## Goal

A sandbox browser whose profile came from another host (an instance move, a restore, a copy)
starts without a person removing Chrome's lock files, and a start that still fails says why.

## Context

The 2026-11-21 instance move (found by task #575) carried every sandbox's browser profile
(`<data>/sandboxes/<id>/browser`) with Chrome's process-singleton entries: `SingletonLock` (a
symbolic link to `<hostname>-<pid>`), `SingletonCookie` and `SingletonSocket` (links to a socket
under the old host's `/tmp`). Chrome refuses a profile whose lock names another host. Measured
with Google Chrome 158 in this sandbox:

- headless (`--headless=new`): it prints "The profile appears to be in use by another Google
  Chrome process (4242) on another computer (old-host). Chrome has locked the profile…" and
  exits 21;
- headful on a virtual display (how Yulius runs it): it prints the same line and shows a dialog,
  never answering on its DevTools pipe.

`BrowserManager::launch` (`crates/yulius-browser/src/manager.rs`) then waits the 30 s startup
timeout, kills Chrome and stops the display; the next viewer or agent tool call starts both
again, so the display was restarted every 30 s with only "Chromium did not answer on its
DevTools pipe within 30s" to show for it. For a lock of this host Chrome heals itself (a dead
pid or a pid that is not Chrome: it removes the lock), and a live Chrome of this host gets the
new window handed over ("Opening in existing browser session.", exit 0).

`prepare_profile` only creates the directory (mode 0700). Nothing clears a lock. Related:
[SPEC-570](SPEC-570-restart-the-locked-browser-of-one-sandbox.md) restarts one sandbox's
browser; the move helper is `crates/yulius/src/instance/migrate`.

## Requirements

- **SPL-001** Before starting Chrome on a profile, Yulius removes the profile's `SingletonLock`,
  `SingletonCookie` and `SingletonSocket` when no running browser can hold it: the lock names
  another host; it names this host with a pid that is not running; it is not a link of the form
  `<host>-<pid>`; or there is no lock (an orphaned cookie or socket link). Links are removed
  themselves, never followed (their targets are gone after a move).
- **SPL-002** A lock naming this host with a running pid is kept with its cookie and socket, so
  a browser Yulius runs (or any live Chrome) never loses its profile lock; Chrome itself decides
  (hand-over, or its own removal when the pid is not Chrome).
- **SPL-003** When a start fails while such a kept lock names a running process, the failure
  says the profile is held by that process on this host. When a stale lock cannot be removed,
  the start fails before the display or Chrome starts, saying which lock and why.
- **SPL-004** The fake Chromium behaves like the real one: it holds its own `SingletonLock`
  (`<host>-<pid>`) while it runs, refuses a profile whose lock names another host (headless:
  exit 21; on a display: it never answers), and hands over to a live lock of this host (exit 0).

## Plan

- [ ] Spec and decision record for clearing stale profile locks
- [ ] Fake Chromium holds and honours profile locks like Chrome (failing tests first)
- [ ] Stale profile locks are cleared before Chrome starts; a live one is kept
- [ ] A start that fails on a held profile says which process holds it
- [ ] Real Chrome starts on a profile carried from another host (opt-in test)
- [ ] Operations notes, architecture and changes updated
- [ ] Independent review in a fresh session, findings fixed test-first

## Acceptance criteria

- **AC-1 (SPL-001, SPL-004)** A profile with `SingletonLock -> old-host-4242` and dangling
  `SingletonSocket`/`SingletonCookie` links: the fake Chromium refuses it without the fix; with
  it `ensure` starts the browser and the lock then names this host and the browser's pid.
- **AC-2 (SPL-001)** A lock of this host with a pid that is not running, a lock that is not a
  `<host>-<pid>` link, and cookie/socket links without a lock are all removed before the start.
- **AC-3 (SPL-002, SPL-003)** A lock of this host naming a running process is kept (all three
  entries unchanged); the fake hands over and exits, and the error names the holding pid.
  A second sandbox pointed at the profile of a running browser leaves that browser's lock alone.
- **AC-4 (SPL-003)** A stale lock that cannot be removed fails the start with a profile error
  naming the lock, and no display is started.
- **AC-5** Real Chrome (opt-in, `YULIUS_TEST_CHROMIUM`) starts headful on a virtual display with
  a profile locked by another host.

## Test plan

Unit tests of the lock reading and the decision (`crates/yulius-browser/src/profile.rs`);
fake-Chromium manager tests (`crates/yulius-browser/tests/manager.rs`) for AC-1..AC-4 written
before the fix and seen failing; an opt-in real-Chrome test in
`crates/yulius-browser/tests/real_chromium.rs` run here with Google Chrome on Xvfb (CI does not
run it). `scripts/dev quick`, the crate's tests, and Yulius CI for the whole gate.

## Decisions

- Clear locks at the one place every start passes (`prepare_profile`, before the display and
  Chrome start), not in the move helper: a profile also arrives by restore or copy, and a move
  that cleared it would still leave every other path broken. Recorded as
  [ADR-0432](../../docs/decisions/0432-a-browser-start-clears-chrome-profile-locks-no-live-process-holds.md).
- Decide like Chrome does from the lock's `<host>-<pid>` (split at the last `-`) and the host
  name of `gethostname`, which Chrome itself compares; a running pid of this host keeps the lock
  even when it might be another program reusing the pid: Chrome resolves that case itself, and
  Yulius never breaks a live browser's lock.
- A start on a profile held by a running local process is still attempted (Chrome may find the
  pid is not Chrome and proceed); only its failure is explained, so a reused pid never blocks
  the browser.
- `gethostname` comes from `nix` (feature `hostname`), already a dependency; no new crate.
