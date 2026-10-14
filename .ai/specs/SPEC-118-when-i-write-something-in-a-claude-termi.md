---
yulius-task: 04be3bab-2271-45f2-aded-78f1d0f469d7
status: done
---

# SPEC-118: A keystroke reaches the terminal without waiting for a permission check

Status: implemented
Date: 2026-10-14
Owner: Yulius task #103
Scope authorization: task #103 ("When I write something in a claude terminal or shell terminal, I
get delay. Things should be written instantly and show up while it's streamed to the engine").
Refines SPEC-002 §7.7 (terminal WebSocket) and AC-ACC-5; keeps SPEC-070 R3 (only the owner types).

## Goal

What a person types into a task terminal (Claude Code) or a shell shows up as fast as the agent or
shell echoes it. A keystroke never waits for the database or GitHub, and the terminal's output never
pauses while access is re-checked.

## Context

**Symptom.** Typing into a task terminal or a shell lags: characters appear late, sometimes in
bursts.

**Path of a keystroke.** xterm `onData` → `TerminalSocket.sendInput` (one JSON frame, no batching) →
the server's per-viewer loop (`routes/terminal.rs`, `routes/shells.rs`) → `App::terminal_input` /
`App::shell_input` → the ptyhost's Unix socket → the PTY. The echo goes back the same way: ptyhost
output pump → `Hub::output` (broadcast) → the same per-viewer loop → the browser. Accepted TCP
connections already set `TCP_NODELAY` (#29); the client, ptyhost and hub forward at once.

**Root cause.** The per-viewer loop handles one thing at a time, and two of them wait on slow work:

1. Every input frame re-checks write access before it is written: `terminal_input` calls
   `can_work` (task, sandbox and cached GitHub level: three database reads) and `shell_input` calls
   `shell_still_allowed` (sandbox, cached level, task). `resize`, `active` and `take_control` do the
   same. The store is one SQLite thread shared by the whole server (`yulius-store/src/lib.rs`: CI
   logs, transcript sync and indexing, metrics, every API call), so each keystroke queues three
   jobs behind everything else. When the 5-minute GitHub permission cache has expired, the
   keystroke waits for a GitHub API round trip too.
2. The 60 s access re-check runs inside the same `select!` loop. While it waits for the database
   (and, every fifth minute, for GitHub), the loop neither forwards output nor reads input: the
   terminal freezes, then catches up in a burst.

The secret redactor (VLT-007) holds back only a tail of at least 4 bytes that begins a secret
value, so it does not delay ordinary echo; it is out of scope.

**What the per-keystroke check bought.** The GitHub level was already cached for 5 minutes, so a
downgrade on GitHub reached input only after that. What it made immediate were facts in the
database: a task handed to another owner (SPEC-070: `a_task_shell_stops_taking_input_once_the_task_is_handed_over`),
a removed sandbox, a disabled user. Those must stay immediate.

## Requirements

- KEY-001: An input frame from a viewer whose write access was confirmed reaches the PTY without a
  database read or a GitHub call, while nothing that decides access has changed since.
- KEY-002: The store keeps an in-memory access epoch that grows with every write that can change
  who may type: a task's owner (hand-over, take-over, delete), a sandbox's removal, a user's
  disabling, instance-admin flag or GitHub link, a session or API token revocation, a cached GitHub
  level that changed, a user's cleared permission cache. A grant confirmed at an older epoch is
  checked again before the next input is written, so these changes still refuse the very next
  keystroke.
- KEY-003: A grant older than 30 s is re-confirmed in the background while input keeps flowing; a
  refusal there refuses the following input. A GitHub downgrade still reaches input once the
  permission cache expires (AC-ACC-5).
- KEY-004: `resize`, `take_control` and `active` use the same grant as input.
- KEY-005: The periodic re-check (sign-in, view access, write access; `viewer_recheck_ms`, 60 s)
  runs beside the viewer's loop: output and input keep flowing while it waits on the database or
  GitHub, and at most one runs per viewer at a time. Its verdicts are unchanged: signed out or
  access lost → close 4003; write access lost on a task → `state` with `can_write: false`.

## Plan

- [x] Read the product context and related work, then write and validate the spec
- [x] The store counts changes to who may type (an access epoch) and the app keeps a write grant per viewer
- [x] A task terminal keystroke and its echo never wait for a permission check, across re-checks
- [x] A shell keystroke and its echo never wait for a permission check; a hand-over still refuses the next one
- [x] An old grant is re-confirmed beside the input, and a downgrade found there refuses the next keystroke
- [x] ADR, architecture, API and SPEC-002 text say how terminal input is authorized
- [x] Independent review in a fresh session, findings fixed test-first
- [x] Every gate green, docs and the changes log updated

## Acceptance criteria

- With GitHub hanging and the permission cache expired, a task terminal writer's keystroke is echoed
  (it previously waited for GitHub forever).
- With GitHub hanging and the permission cache expired, a shell keystroke is echoed.
- With the re-check interval at its floor and GitHub hanging, a task terminal keeps carrying output
  and input across several re-check ticks.
- Existing access tests stay green unchanged: a read-only viewer's input is refused, a GitHub
  downgrade reaches open sockets once the cache expires, a viewer without access is closed with
  4003, a task shell refuses input at once after the task is handed over.
- Unit tests: the store's epoch grows on each listed write and not on a `put_access` with the same
  level; the grant decides check / allow / allow-and-refresh as KEY-001..003 say.

## Test plan

- `crates/yulius-store/tests/store.rs` `writes_that_change_access_grow_the_access_epoch` (KEY-002).
- `crates/yulius-app/src/write_grant.rs` unit tests: check first, allow while the epoch holds,
  re-confirm once beside the input when old, check again after a refusal (KEY-001, KEY-003).
- `crates/yulius/tests/terminal_ws.rs` `keystrokes_and_echo_never_wait_for_a_permission_check`:
  input, `resize` and `active` with GitHub hanging, the cache expired and the 300 ms re-check
  ticking (KEY-001, KEY-004, KEY-005).
- `crates/yulius/tests/shells.rs` `shell_keystrokes_never_wait_for_a_permission_check` (the same for
  a shell) and the existing `a_task_shell_stops_taking_input_once_the_task_is_handed_over` (KEY-002).
- `crates/yulius/tests/task_ownership.rs` `a_hand_over_refuses_the_previous_owners_next_keystroke`,
  `disabling_the_typist_refuses_their_next_keystroke`, `signing_out_refuses_the_next_keystroke`
  (KEY-002) and `a_downgrade_found_beside_the_input_refuses_the_next_keystroke` (KEY-003).
- `write_grant.rs` `input_flows_while_a_re_confirmation_is_pending` and
  `a_re_confirmation_that_fails_checks_the_next_input` (a failed re-confirmation drops the grant).
- Not covered by a test: a periodic re-check that panics closes the viewer (no way to inject the
  panic without a test-only hook); reviewed by reading.
- The fake GitHub gains a `hang_permissions` switch: `repo_access` never answers while it is on.

## Decisions

- ADR-0096: terminal input is authorized by a per-viewer write grant tied to the store's access
  epoch, re-confirmed in the background; the periodic re-check runs beside the viewer's loop.
  Alternatives (drop the per-input check and rely on the 60 s re-check; a separate database
  connection for reads; a client-side local echo) are compared there.

## Evidence

- Reproduction (before the fix): `terminal_ws::keystrokes_and_echo_never_wait_for_a_permission_check`
  failed with `"key 0" was not echoed within 5 s while permission checks hang`, and
  `shells::shell_keystrokes_never_wait_for_a_permission_check` with `key-100 was not echoed within
  5 s`. Both pass after it.
- New tests that failed first: the store's epoch test (no `access_epoch`, then a first cached
  level that moved it), `disabling_the_typist_refuses_their_next_keystroke` (the keystroke was
  allowed), `input_flows_while_a_re_confirmation_is_pending` (a guard captured by field settled at
  once). A mutation that skipped the background re-confirmation made
  `a_downgrade_found_beside_the_input_refuses_the_next_keystroke` fail.
- Existing access tests are unchanged and green: the read-only refusal, the GitHub downgrade and the
  4003 close in `terminal_ws`, and the shell hand-over in `shells`.
- `scripts/dev ci` on the head after merging `origin/dev`: exit 0, with fmt, clippy `-D warnings`,
  deny, shear and docs. nextest ran 1617 tests: 1617 passed, 16 skipped.
- Independent review: round 1 requested changes. A disabled or signed-out typist was not refused at
  once. A failed re-check counted as fine. A panicking re-confirmation stuck. A first cached level
  moved the epoch. The interval could burst. Round 2 approved with minors (a signed-out test, an
  unused method, wording), and those are fixed.
- Not tested in this environment: typing latency against the production server's real database
  load and real GitHub. The fix removes the waits on both, and the hanging-GitHub tests prove a
  keystroke no longer waits on them.
