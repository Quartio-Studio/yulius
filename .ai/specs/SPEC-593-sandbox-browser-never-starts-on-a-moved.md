---
yulius-task: 2bfb2cd1-d319-4db4-bbb7-e1467fffa1a0
status: done
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
[SPEC-570](SPEC-570-restart-the-locked-browser-of-one-sandbo.md) restarts one sandbox's
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
- **SPL-003** When Chrome gives up on a start (it exits, or never answers on DevTools) while
  such a kept lock still names the same running process, the failure says the profile is held
  by that process on this host; other failures (the display, a lock Chrome took over) do not.
  When a stale lock cannot be removed,
  the start fails before the display or Chrome starts, saying which lock and why.
- **SPL-004** The fake Chromium behaves like the real one: it holds its own `SingletonLock`
  (`<host>-<pid>`) while it runs, refuses a profile whose lock names another host (headless:
  exit 21; on a display: it never answers), hands over to a running fake browser's lock of this
  host (exit 0) and takes over a lock whose running pid is no browser.

## Plan

- [x] Spec and decision record for clearing stale profile locks
- [x] Fake Chromium holds and honours profile locks like Chrome (failing tests first)
- [x] Stale profile locks are cleared before Chrome starts; a live one is kept
- [x] A start that fails on a held profile says which process holds it
- [x] Real Chrome starts on a profile carried from another host (opt-in test)
- [x] Operations notes, architecture and changes updated
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Delta review of the round-2 fixes in a fresh session

## Acceptance criteria

- **AC-1 (SPL-001, SPL-004)** A profile with `SingletonLock -> old-host-4242` and dangling
  `SingletonSocket`/`SingletonCookie` links: the fake Chromium refuses it without the fix; with
  it `ensure` starts the browser and the lock then names this host and the browser's pid.
- **AC-2 (SPL-001)** A lock of this host with a pid that is not running, a lock that is not a
  `<host>-<pid>` link, and cookie/socket links without a lock are all removed before the start.
- **AC-3 (SPL-002, SPL-003)** A lock of this host naming a running process is kept (all three
  entries unchanged). A second sandbox pointed at the profile of a running browser leaves that
  browser's lock alone, and its failed start names the holding pid. A lock naming a running
  process that is no browser does not block the start: Chrome takes it over.
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
- Read the lock as Chrome does: `<host>-<pid>` split at the last `-`, the host compared with
  `gethostname`. Yulius differs from Chrome in one case: Chrome also keeps another host's lock
  when its pid is a running Chrome here (a renamed host shared by two starters), which Yulius
  removes; the manager never starts two browsers on one profile. A running pid of this host keeps
  the lock even when another program may have reused the pid: Chrome resolves that case itself,
  and Yulius never breaks a live browser's lock.
- A failed start is blamed on the holder only when Chrome's DevTools wait failed (it exited, or
  never answered) and the lock still names the same pid afterwards (review rounds 1 and 2):
  Chrome replaces a lock whose pid is no Chrome with its own, so a reused pid is named only when
  Chrome died before it reached its lock check (a missing library); the error keeps Chrome's own
  failure beside it.
- A start on a profile held by a running local process is still attempted (Chrome may find the
  pid is not Chrome and proceed); only its failure is explained, so a reused pid never blocks
  the browser.
- `gethostname` comes from `nix` (feature `hostname`), already a dependency; no new crate.

## Evidence

- Real Chrome 158.0.8127.36 in this sandbox, profile with `SingletonLock -> old-host-4242`:
  headless exited 21 with "The profile appears to be in use by another Google Chrome process
  (4242) on another computer (old-host)"; headful on Xvfb it printed the same and never answered
  (killed by `timeout` after 12 s).
- Test-first: the four new manager tests failed before the fix, for the intended reasons:
  `ExitedEarly { status: 21, "…another computer (old-host)" }` (AC-1), `ExitedEarly { status: 0,
  "Opening in existing browser session." }` not naming the holder (AC-3, twice), and
  `StartupTimeout { 30s }` with the display started (AC-4). Review round 1's tests failed before
  their fix (a `sleep` holder refused the start; a refused fake recorded its pid), as did round 2's
  (an Xvfb exiting early beside a held lock was reported as `ProfileInUse`). Replacing the
  `still_held` check with "always" fails `a_start_that_failed_for_another_reason…`.
- `cargo nextest run -p yulius-browser --features fake`: 150 passed. `cargo clippy -p
  yulius-browser --features fake --all-targets -- -D warnings`: clean. `scripts/dev quick`: passed.
- `yulius` crate suites `browser::`, `superagent_browser::` (and `browser_network::`,
  `live_display::` on the first head): 41 and 43 passed.
- Opt-in real Chrome (`YULIUS_TEST_CHROMIUM=/usr/bin/google-chrome
  YULIUS_TEST_CHROMIUM_NO_SANDBOX=1`): `real_chrome_starts_on_a_profile_locked_by_another_host`
  passed (headful on Xvfb, about 2 s; the lock then names this host and Chrome's pid); with the
  cleanup disabled it failed with `StartupTimeout { 60s }`, the production loop. The whole
  opt-in suite with a `--no-sandbox` wrapper: 5 of 6 passed; the sixth,
  `a_secret_is_not_typed_into_a_tab_that_is_leaving…`, flakes on Chrome 158 on a fresh profile
  without locks (1 failure in 4 runs, `ElementNotFound`, nothing typed), outside this change:
  [#1604](https://github.com/Quartio-Studio/yulius/issues/1604). CI never runs these tests.
- CI run e822c548 failed `e2e` in `claude-connect.e2e.ts` (a mixed request refused): the mock's
  ambient browser request for #10 arrives 5 s in and became a second toast. A 6 s delay probe
  before the toast check reproduced it locally; with `ambient=0` the file passed 7/7 with the
  probe and without it.
- Independent review: round 1 (Fable 5.2) approved `fbd738d9e` with three minors and three nits;
  round 2 approved `c31c4e3a2` with two minors and three nits; all fixed test-first above.
  Round 3 (Opus 5.5, delta of the round-2 fixes) approved `e34016b0d` with nits only: two long
  document lines, wrapped here, and a holder `sleep` a failed test setup could leave running
  (the same pattern as the other SPEC-593 tests; it only matters on a setup failure). The
  pre-existing note that an Xvfb exit reads "Chromium exited during startup"
  (`display.rs`) is outside this change.
- `scripts/dev security`: Vidoc is switched off for this project (CI `security` skipped), so
  unrun, not passed.
- Not tested here: an actual instance move between two hosts (the real-Chrome test stands in for
  the carried profile); the move helper is unchanged by decision.
