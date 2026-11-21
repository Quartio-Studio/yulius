# 0432. A browser start clears Chrome profile locks no live process holds

## Status

Accepted for [SPEC-593](../../.ai/specs/SPEC-593-sandbox-browser-never-starts-on-a-moved.md).

## Context

Chrome guards a profile with `SingletonLock`, a symbolic link to `<hostname>-<pid>`, beside
`SingletonCookie` and `SingletonSocket`. A profile carried to another host (the 2026-11-21
instance move) keeps them. Chrome refuses a profile locked by another host: headless it exits 21,
headful on a virtual display it shows a dialog and never answers on DevTools, so each start
waited 30 s, was killed with its display, and started again on the next use.

## Decision

Before every start, `prepare_profile` reads the lock and removes the three entries (links
themselves, never their targets) when no running browser can hold it: the lock names another
host, names this host with a pid that is not running, is not a `<host>-<pid>` link, or is absent
while a cookie or socket link remains. A lock of this host with a running pid is kept and Chrome
decides; a start whose DevTools wait failed (Chrome exited or never answered) while the lock
still names that pid names the holding process, beside Chrome's own failure. Unlike Chrome,
Yulius also removes another host's
lock whose pid happens to be a running Chrome here; the manager never starts two browsers on
one profile. A stale lock that cannot be removed
fails the start before the display starts.

## Alternatives

- Clear the locks in the move helper only: a restore or a copy would still break, and the
  browser would still loop on any other carried profile.
- Pass a flag to Chrome to skip the check: there is none for the foreign-host case; the dialog
  is the only way past it.
- Remove the locks unconditionally: a live Chrome using the profile (one Yulius runs, or a
  person's) would lose its guard and the profile could be corrupted.
- Refuse a start while a live local pid holds the lock: a pid reused by another program would
  block the browser forever, where Chrome itself would proceed.

## Consequences

A moved, restored or copied profile starts without a person. The host name is the one Chrome
compares (`gethostname`); a new host with the old host's name is handled by Chrome's own
same-host rules. The fake Chromium holds and honours locks like Chrome, so tests sharing a
profile between two running browsers now see Chrome's hand-over.
