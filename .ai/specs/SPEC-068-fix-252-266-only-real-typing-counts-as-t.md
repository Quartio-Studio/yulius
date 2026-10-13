---
yulius-task: a12a5e62-8e55-45dc-ae03-b27e430817f8
status: done
---

# SPEC-068: Only real typing counts as typing; a typed first message is never lost

Status: implemented
Date: 2026-10-13
Owner: Yulius task #61
Scope authorization: task #61 (part of task #47, "fix all open actionable GitHub issues without
creating new issues and close them"): fix GitHub issues #252 and #266 following their acceptance
criteria. Extends [SPEC-025](SPEC-025-uploaded-attachements-should-be-clickabl.md) (ATT-003) and
[SPEC-030](SPEC-030-i-asked-previous-task-to-attach-images-a.md) (PIMG-002,
[ADR-0026](../../docs/decisions/0026-attached-images-are-pasted.md)).

## Goal

Yulius counts only a person's keystrokes as typing, so a goal task leaves a person's draft alone
instead of appending `/goal` to it; and the first message Yulius types into a session with images
is never silently lost by a server restart or carried without its images after a capacity hold.

## Context

- `App::terminal_input` (`crates/yulius-app/src/terminal.rs`) claims control and calls
  `record_typing` for every input frame that is not a forwarded wheel turn. The web terminal
  forwards everything xterm.js emits (`term.onData`): focus reports (`ESC [ I` / `ESC [ O`, sent
  because Claude Code enables focus reporting), mouse reports (SGR `ESC [ < b ; x ; y M|m`, X10
  `ESC [ M` + 3 bytes) and xterm's answers to the program's queries (device attributes
  `ESC [ ? … c`, cursor position `ESC [ r ; c R`, mode reports `ESC [ ? … $ y`, OSC colour and
  DCS version answers). All of them count as typing today (#252).
- `spawn_goal` (`supervisor.rs`) reads `typed_at_ms` only after the opening turn (SPEC-025): a
  draft written during that turn gets `/goal …` appended and submitted. Skipping the goal on
  "a person typed" was not safe while a click counted as typing (SPEC-025 Decisions, #252).
- SPEC-030: a session with attached images starts without a CLI prompt and Yulius types its first
  message (pastes, then Enter, `type_opening`) from a task in the old server's process (#266):
  - `reconcile` reattaches a live run after a restart but nothing types the message again: the
    session waits at an empty or half-filled prompt while the task reads Working.
  - A first message held for capacity (`UserPromptSubmit`, `PauseKind::Prompt`) keeps the hook's
    prompt text (`[Image #1] Make it blue`) as the pause's continuation; resumed on another
    account (`switch_task_now`) the new session gets that text without the image.
- The hook client fails open when Yulius is unreachable (`hook_client.rs`): a prompt submitted
  while the server is down reaches the agent without Yulius knowing.

## Requirements

| ID | Requirement | Evidence |
| --- | --- | --- |
| TYP-001 | An input frame is typing when it holds at least one keystroke: text, a control key (Enter, Backspace, Tab, Ctrl-…), an Escape key sequence (arrows, Delete, Home, function keys, Alt-…) or a bracketed paste. Focus reports, SGR and X10 mouse reports and terminal answers (device attributes, cursor position, mode reports, OSC and DCS strings) are not. | `terminal.rs` unit test |
| TYP-002 | `terminal_input` claims control and records typing only for typing input; other input is still sent to the session. The web terminal takes the size locally only for typing input (`typing.js`, the same rule). | `first_message.rs` focus test; e2e `terminal.e2e.ts` |
| TYP-003 | A goal task whose terminal a person typed into after its run was attached and before Yulius types `/goal` gets no `/goal`: the draft is left alone, the task is flagged NeedsAttention and its terminal gets a notice saying the goal was not set. | `first_message.rs` draft test |
| OPN-001 | A run whose first message Yulius types records that in its run directory (`opening-pending`, empty) before the session starts. Any `UserPromptSubmit` that reaches Yulius marks it `entered` before any hold; the first one let through, or any Stop, removes it. | `first_message.rs` restart, update and hold tests; `subscription_pool.rs` Stop test |
| OPN-002 | After a server restart, a reattached run whose first message never reached the submit hook (pending, not `entered`) and that is not held for capacity is flagged NeedsAttention, with a notice in its terminal saying the first message was never sent (and, in goal mode, that the goal is not set). A submitted first message an update hold keeps is not flagged and goes on after the restart; in goal mode the new server types the goal after that message's turn. | `first_message.rs` restart and update tests |
| OPN-003 | A first message held for capacity and resumed on another account (or restarted without a process) starts a fresh session that types the task's own opening (`Request::opening`), images pasted, instead of resuming with the hook's prompt text. A manual account switch of a task whose first message is pending does the same. Released on the same account, the held message goes on as it is. | `first_message.rs` hold and manual switch tests |

## Acceptance criteria

- AC-TYP-1: focus in/out and SGR/X10 mouse reports are not typing; text, Enter, Backspace and a
  bracketed paste are (unit).
- AC-TYP-2: during the opening turn of a goal task with an attached file, focus and mouse reports
  sent through `terminal_input` do not stop the goal: `/goal` is typed once after the turn.
- AC-TYP-3: a draft typed through `terminal_input` during that opening turn does not get `/goal`
  appended; the task is flagged NeedsAttention.
- AC-OPN-1: the server restarts while a first message with an image is being typed; the new
  server flags the task NeedsAttention with a notice, and nothing is submitted for it.
- AC-OPN-2: the first message of a task with an image is held for capacity and resumed on
  another account; the new session gets `[Image #1] Make it blue`.

## Plan

- [x] Only keystrokes count as typing in the task terminal
- [x] A goal task leaves a person's draft alone and flags the task
- [x] A first message never sent before a restart flags the task
- [x] A held first message resumes elsewhere with its images
- [x] Docs, ADRs and changes updated; independent review and gates green

## Test plan

- Unit (`terminal.rs`): `is_typing` on focus in/out, SGR press and release, X10 press, device
  attributes, cursor position and mode report answers, OSC and DCS answers (not typing); text,
  Enter, Backspace, an arrow key, Escape, a bracketed paste, and a focus report followed by a key
  (typing).
- Integration (`crates/yulius/tests/first_message.rs`, fake agent through `yulius ptyhost` with
  Claude Code's hooks through the real hook client):
  - focus and mouse reports during the opening turn (`--session-start --opening-turn-ms`): the
    goal is set once (the fake agent drops focus and mouse reports like Claude Code);
  - a draft typed during that turn: no `/goal`, the task NeedsAttention, the notice sent;
  - a server restart after the image paste reached the terminal and before Enter: the new
    server flags the task NeedsAttention with a notice, and the agent echoed nothing;
  - two accounts; the only one with capacity is exhausted before the first message is
    submitted, so it is held; the other recovers and the resume round moves the task: a fresh
    session (`launch_kind` new, no prompt) whose agent echoes `[Image #1] Make it blue`.
- Each fails before the fix (checked by running it on the unfixed code).

## Decisions

- Classify input by its escape sequences rather than having the browser filter it: the server
  is the one that decides what typing means (any client, also the `yulius` CLI, goes through
  `terminal_input`), and the browser cannot see every report xterm.js answers by itself. Not
  typing also means no control claim: a viewer's focus report or a terminal answer must not take
  the size from the person typing. Recorded as ADR-0050.
- A goal task with a draft gets no goal and is flagged (the issue's choice) rather than
  waiting for the draft to be submitted: a draft may never be submitted, and a submitted draft is
  a turn on its own after which the person decides.
- The pending first message is a file in the run directory (`opening-pending`), not a store
  column: it lives exactly as long as the run, survives a restart like the run's other files, and
  needs no migration. Alternatives: a flag in memory (lost on the restart it must survive);
  matching the held prompt text against the request (Claude Code's text formatting is not a
  contract).
- After a restart a pending first message is flagged, not typed again: how much of it the old
  server typed is unknown, and typing again into a half-filled prompt box would duplicate the
  images or send a mangled request. The issue allows either outcome.
- A held first message resumes as a fresh session: the held session has no turn yet, so nothing
  is lost, and a fresh session of a task with images types its opening (SPEC-030) with the
  images, then its goal in goal mode.
- ADR-0026 is amended for both #266 paths.
- A goal task whose submitted first message survives a restart (held by an update or capacity)
  gets its goal typed by the new server after that turn, instead of a flag: the hold ends by
  itself and the task would otherwise run without a goal (review round 2).

## Evidence

- `cargo test -p yulius-app --lib only_keystrokes`: passes; failed against the stub that counted
  every frame as typing (the behaviour before), and again before X10 coordinates were skipped as
  UTF-8 characters.
- `cargo test -p fake-agent`: 14 passed (focus and mouse reports dropped like Claude Code).
- `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^first_message::/)'`: 7
  passed. Before each fix: the draft test, the restart test and the hold test timed out (no
  flag, no flag, no `[Image #1]` in the resumed session); the update test timed out with the
  marker never `entered`; the goal-after-update test with no `GOAL SET`; the manual switch test
  with no first message. The focus test passes before and after: it guards the draft change
  against clicks.
- `subscription_pool::a_stop_ends_a_pending_first_message`: failed before Stop cleared the
  marker, passes.
- `first_message`, `task_done`, `subscription_pool`, `session_sync` and `terminal_ws` together
  after merging `dev` (980059c): 36 passed. The draft test once timed out under that load with a
  6 s opening turn (the draft landed after the turn); the turn is now 12 s and the test asserts
  the draft precedes `DONE opening`.
- `npx e2e run` (full suite): 244 passed, 1 skipped, including "only keystrokes count as typing"
  (failed before `typing.js` existed).
- `scripts/dev ci` on c008bca: exit 0 (repository checks, fmt, clippy `-D warnings`, nextest
  1324 passed, doctests, cargo-deny ok, machete, docs). An earlier run's doctest failure (a
  missing `rustls` rlib) came from the reviewer building in the same target directory at once.
- Review round 1 (fresh session) requested changes: an update hold made the restart flag a
  submitted first message; a stale marker could restart a working session fresh; the browser
  still took the size on focus reports; `typed_before` was read after the run was attached; X10
  coordinates of 96 and up. All fixed test-first.
- Review round 2 approved 0c8937c with minors: a goal task held by an update lost its goal over
  the restart (fixed: the new server types it after the first turn); the manual account switch
  lost a held first message (fixed: fresh session); the marker could be re-created by a race
  (fixed: never created when marking it entered); `typing.js` preloaded with the terminal core.
- Not covered: that `send()` skips the size take for a focus report end to end (the browser test
  covers the classifier only; the wiring is one guarded return); a real Claude Code session
  (needs a paid credential); the fake agent stands for it, measured against Claude Code 2.1.292
  in SPEC-030.
