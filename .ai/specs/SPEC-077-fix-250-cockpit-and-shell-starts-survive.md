---
yulius-task: f92dbbba-5e79-41d6-87b9-e09a3cbb5278
status: done
---

# SPEC-077: Cockpit and shell starts survive a dropped request

Status: implemented
Date: 2026-10-13
Owner: Yulius task #64
Scope authorization: task #64 (part of #47), fix GitHub issue #250.

## Goal

A Cockpit start (`POST /api/sandboxes/{sid}/cockpit`) and a terminal start
(`POST /api/sandboxes/{sid}/shells`) finish, or clean up, whatever the client does: a client that
goes away mid-start never leaves an unregistered process, a stuck terminal or an orphan run
directory, and the next open finds the one Cockpit. Two Cockpit behaviours of SPEC-021 review
round 2 get the regression tests they lack.

## Context

- SPEC-021 (Cockpit, ADR-0020) and UI-015/UI-016 (terminals) start a shell runtime in the request:
  `App::cockpit_open` launches (`launch_cockpit` → `spawn_shell` → `save_shell`) and only then
  `register_shell`s; `App::shell_create` inserts the terminal in the registry first, then
  launches, saves and starts its pump.
- Axum drops a handler's future when its client disconnects. Dropped between the spawn and the
  registration, the Cockpit's process runs and its run directory stays, but it is never
  registered: the next open starts a second Cockpit on the same conversation and both are adopted
  after a restart. A dropped terminal start stays in the registry with no pump: listed, never
  attachable, never reaped by its pump.
- `note_cockpit_exit` marks a resumed conversation unresumable only for an error exit
  (`Some(code != 0)`) within 10 s of the spawn; a signal does not. Cockpit starts are serialised
  per (user, sandbox) by `ShellState::cockpit_start`, a map that never drops its entries. Neither
  behaviour has a test (issue #250, SPEC-021 review round 2).

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| DRS-001 | The section of a Cockpit start from the start lock to its registration runs as a task spawned by the request and awaited by it: when the request is dropped the start still ends registered (or, on an error, discarded with its run directory). The next open returns that same Cockpit. | `cockpit.rs` `a_cockpit_start_whose_request_is_dropped_still_ends_as_the_one_cockpit` |
| DRS-002 | The section of a terminal start from its registration to its pump runs the same way: a dropped request leaves a live, saved terminal (or nothing on an error), never a stuck entry. | `shells.rs` test `a_terminal_start_whose_request_is_dropped_still_ends_live` |
| DRS-003 | A Cockpit ended by a signal right after a resume keeps that conversation: the next start resumes it (`--resume <id>`). | `cockpit.rs` `a_resumed_cockpit_killed_by_a_signal_resumes_the_same_conversation` |
| DRS-004 | A user's Cockpit start that blocks does not delay another user's Cockpit open in the same sandbox, nor the same user's in another sandbox. | `cockpit.rs` `a_blocked_cockpit_start_delays_no_other_user_or_sandbox` |
| DRS-005 | The per-(user, sandbox) start-lock map holds an entry only while a start of that pair runs or waits. | same test, `App::cockpit_starts_pending` |

## Design

- `App::run_detached(start)` (`shells.rs`): `tokio::spawn(start).await`, a panic mapped to
  `Internal`. The spawned future owns what it needs (`Arc<App>`, the `AuthContext` clone, the
  sandbox record, the runtime), so dropping the request only drops the `JoinHandle`, which
  detaches the task ([ADR-0053](../../docs/decisions/0053-shell-starts-run-detached-from-the-request.md)).
- `cockpit_open` checks access and returns a running Cockpit at once as before, then runs
  `start_cockpit` detached: the start lock, the second running check, the clone check, the pool
  account, the launch, the registration and the pump. The start lock is held by the detached task,
  so a second open waits for the in-flight start and then returns its Cockpit.
- The start lock is taken through a guard (`CockpitStartSlot`) holding an owned tokio mutex guard;
  dropping it removes the map entry when nobody else holds that lock (checked under the map's
  lock), whatever way the start ended.
- `shell_create` keeps its checks and registration in the request and runs the launch, save and
  pump (or discard) detached.

## Plan

- [x] Write and validate SPEC-077
- [x] A Cockpit start finishes or cleans up when its request is dropped (test-first)
- [x] A shell start finishes or cleans up when its request is dropped (test-first)
- [x] A signal after a resume keeps the conversation resumable (test)
- [x] One user's blocked Cockpit start does not delay other users or sandboxes (test)
- [x] The start-lock map drops idle entries
- [x] Docs, ADR, changes log and every gate green
- [x] Independent review in a fresh session and its findings fixed

## Acceptance criteria

- A test drops a Cockpit open while its process is spawned but not yet attached; once the start
  is let go, there is one Cockpit, the next open returns it and there is exactly one run
  directory (none orphaned). It fails on the code before this change.
- The same for a terminal: one live terminal with its saved record. It fails before.
- A fake `claude` that kills itself with a signal on `--resume` is followed by a start that
  resumes the same conversation; with the rule changed to "any non-zero or signal exit" it fails.
- While alice's Cockpit start is held, carol's open in that sandbox and alice's in another
  sandbox return; alice's then completes. After every start the lock map is empty.

## Test plan

- Integration, `crates/yulius/tests/cockpit.rs` and `shells.rs` with the real detached ptyhost
  wrapped in a session host whose next `attach` waits for the test (`GatedHost`): drop the
  request future once the gate is reached, release, wait for the condition (no sleeps).
- Signal case with the existing fake `claude` extended by a `kill-resume` marker.
- Mutation check by hand for DRS-003 (treat a signal as a failure) to prove the test bites.

## Decisions

- **Spawn and await rather than a drop guard** (ADR-0053): a drop guard can only discard, so a
  client that goes away would cost the user their start and leave a process to kill
  synchronously from `Drop`; a detached task finishes the work the user asked for and reuses
  the existing error paths (`discard_shell`). The cost is one task per start.
- **The Cockpit's whole start runs detached, including the lock**: the lock must outlive the
  request, or a second open could start while the first one's detached registration is pending.
- **The terminal's checks stay in the request**: a refused name or a full sandbox answer at once;
  only the part after the registry insert can leave a stuck entry.
- **Lock map entries removed by a guard**: an entry is removed only when its `Arc` has no other
  holder under the map lock, so a waiting start always finds the same mutex. Exposed as a count
  (`cockpit_starts_pending`) for the test.

- **Review round 1** (fresh read-only session, APPROVE at ef453a5 with minors): added a second open
  of the same user and sandbox while the first start is held (it waits and gets the same
  Cockpit) and the lock-map check after the concurrent opens; ADR-0053 now says a start that a
  server shutdown cancels is adopted if saved or cleaned up at the next start; a test comment no
  longer claims an untested restart. Not changed: a panic inside the detached start does not
  discard the shell (as before; no code there panics), recorded in ADR-0053.

## Evidence

- Before the change (tests written first, `cargo nextest run -p yulius -E 'binary_id(yulius::main)
  & (test(/^cockpit::/) | test(/^shells::a_terminal_start_whose/))'`): the dropped Cockpit start
  timed out waiting for its registration, the dropped terminal start never saved its record
  (timed out), and the lock map held 3 entries after the starts (`left: 3, right: 0`).
- After the change: `cockpit::` and `shells::` tests, 21 passed (4 new).
- DRS-003 mutation check: with `note_cockpit_exit` treating any exit but `Some(0)` as a failure,
  `a_resumed_cockpit_killed_by_a_signal_resumes_the_same_conversation` fails
  (`[--model …]` instead of `[--resume kept --model …]`); restored, it passes.
- `scripts/dev ci` (repository contracts, script tests, check, fmt, clippy `-D warnings`, nextest,
  doctests, deny, shear, doc) on the head merged with `dev` (9146803): exit 0, 1352 tests run,
  1352 passed, 16 skipped.
- Not verified here: a real browser dropping the request (the tests drop the request future,
  which is what axum does when the client disconnects) and a contained (bwrap) Cockpit.
