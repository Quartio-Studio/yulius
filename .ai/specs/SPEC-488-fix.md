---
yulius-task: e68c4e9a-3bfb-42b0-bf5d-e6fa5aeec9dc
status: in-progress
---

# SPEC-488: A key that left nothing in the prompt box holds no message

Status: accepted
Date: 2026-11-13
Owner: Yulius task #470
Scope authorization: the request of task #470 (by MrFishPL, 2026-11-13): a screenshot of a
finished task flagged "Needs you: Yulius has a message for this session and does not type it:
someone typed in the terminal and sent no prompt since, so their text may still be in the
prompt box", with the word "fix"; issue #1260. Amends SPEC-480 (DRF-001, DRF-006, DRF-007) and ADR-0309;
decision in [ADR-0329](../../docs/decisions/0329-a-prompt-box-the-screen-shows-empty-holds-no-message.md).

## Goal

A message Yulius types into a running session is not held back, and its task is not flagged
Needs your action, for a keystroke that left no text in the session's prompt box. A message is
still never typed behind text that is there (SPEC-480).

## Context

- **Symptom.** A task whose work is done and whose pull request has news reads "Needs your
  action": Yulius holds a message because "someone typed in the terminal and sent no prompt
  since". Nothing is in the prompt box. The person is asked to send a prompt to a session that
  needs none from them, and until they do no pull request's news, no "Yulius is back; continue"
  and no `/mcp reconnect yulius` reaches the session.
- **Root cause.** SPEC-480 (merged 2026-11-12) holds every typed message while a person typed
  after the prompt box was last known empty, and "known empty" is only the run's start or a
  prompt submitted from the box (`policy::box_held`, `App::type_into_session` in
  `crates/yulius-app/src/task_message.rs`). Every key of a viewer counts
  (`App::terminal_input`), also one that leaves no text: Escape to interrupt, an answer to a
  dialog, an arrow or page key, a command Claude Code runs without a prompt hook (`/usage`,
  `/model`), text that was typed and deleted again. SPEC-480 knew this and accepted it as "the
  price of not guessing" (ADR-0309, Consequences); it rejected reading the box from the screen
  as fragile, without having looked at what the real tool draws. Before SPEC-480 such a key
  held messages for a minute; since then it holds them until a person sends a prompt, which an
  autonomous task's person has no reason to do.
- **Why the tests missed it.** SPEC-480's tests type text that stays ("half a thought") and
  none types a key that leaves nothing. The fake agent's default drawing has no prompt box, so
  no test could tell an empty box from a filled one.
- **What the real tool draws** (Claude Code 2.1.323, full screen as Yulius runs it
  (`CLAUDE_CODE_NO_FLICKER=1`), in a PTY against a local stub of the model API, rendered with
  the `vt100` crate the hub uses; scripts and runs in `$HOME/.cache/yulius-task-470`). The
  prompt box is one row `❯` between two rows of `─` across the whole width, on the alternate
  screen, and the terminal's cursor is parked at the box's first cell (column 2 of that row)
  while the box is empty. What was seen:
  - empty box (fresh, mid-turn, after a turn, after Escape idle and mid-turn, after a declined
    permission dialog, after `Ctrl+U`, with the terminal unfocused, 120 and 50 columns wide,
    with the shortcut help open below it): `❯` alone, cursor at column 2;
  - a draft: its text after `❯` in normal intensity, the cursor behind it; with the cursor
    moved to the start (`Ctrl+A`) the cursor is at column 2 and the text stays in normal
    intensity, focused or not, idle or mid-turn; a one-character draft under the cursor is
    one inverse cell; a pasted block is `[Pasted text #1 +39 lines]` in normal intensity; a
    draft of several rows (a first row left empty, or wrapped) has more than one row between
    the rules;
  - what Claude Code draws in an empty box by itself: its prompt suggestion (on by default)
    and "Press up to edit queued messages", both **dim** with the cursor at column 2 (the
    first character inverse under the caret while focused, dim when not). A paste into a box
    that shows a suggestion replaces it: the prompt hook got the pasted message alone;
  - other states have no such box at the cursor: shell mode (`!` instead of `❯`), the history
    search (a labelled upper rule and the entry's text), the rewind list, a permission dialog,
    the transcript view, the agents view (its box holds a grey, not dim, placeholder).
- **What exists to build on.** ADR-0309 ends: "a later, proven way to know the box is empty
  only has to move `box_emptied_ms`". The hub keeps each session's screen in a `vt100` parser
  (`Hub`), with cell attributes and the cursor; a reattached run's hub is rebuilt from the
  process host's snapshot, which keeps the alternate screen, the attributes and the cursor.
  The fake agent's `--tui` draws that box (rule, `❯`, rule, cursor at column 2), and the PTY
  echoes what is typed into it, so a draft shows and `Ctrl+U` clears it.
- **The pull-request watch stops at a flag.** It types nothing into a task that reads Needs
  your action (`person_must_act`), also when the flag is the held message's own. A flagged
  task therefore never has its hold looked at again, and stays flagged until a prompt.

## Requirements

- EBX-001 (the screen proves an empty box). `type_into_session` no longer holds a message for
  a possible draft (SPEC-480 DRF-001) when the session's screen shows Claude Code's prompt box
  with nothing typed in it and the person's last keystroke is more than a minute old. It then
  types as into any session.
- EBX-002 (what counts as shown empty). Exactly what the probes saw: the alternate screen; the
  cursor in column 2 of a row that starts with `❯` and a blank; the rows directly above and
  below consist of `─` across the whole width; and from column 2 on that row holds nothing, or
  only dim text (Claude Code's own suggestion or hint): every non-blank cell is dim, except
  that the cell under the cursor may be inverse when a dim cell follows it. Anything else is
  not proof, and the hold stays as SPEC-480 has it: text in normal intensity, a second row in
  the box, another prompt character, a labelled rule, a dialog or another view, the main
  screen, a withheld screen, a Claude Code that draws differently.
- EBX-003 (the minute stays). A keystroke of the last minute holds a message as before
  (SPEC-480 DRF-002), whatever the screen shows: the person is at the keyboard, and the screen
  may be behind their key.
- EBX-004 (the hold ends). A box seen empty counts as emptied at the keystroke that was judged:
  the draft's mark leaves the run folder and the flag of a held message ends (SPEC-480
  DRF-007, DRF-009), as after a prompt submitted from the box. A key typed after that is a
  possible draft of its own. This also holds for a draft known from its mark alone (after a
  restart of the server): the screen is what the session shows now.
- EBX-005 (a flag of a held message does not stop the pull-request watch). A task flagged only
  because a message waits for a possible draft, whose screen shows the prompt box empty, still
  gets the watch's tries, so a box that became empty (a person cleared it, or the flag is older
  than this change) releases the news and ends the flag without a prompt. Every other reason a
  person must act, and this flag over any other screen, stops the watch as before.
- EBX-006 (nothing is loosened). With text in the box that the screen shows, a message is
  refused, its sender told, and a message of Yulius's own flags the task after ten minutes,
  exactly as SPEC-480 requires.
- EBX-007 (documents). ADR-0309, SPEC-480, SPEC-230 TMS-004, `docs/operations.md`,
  `docs/architecture.md` and `docs/product.md` say that a prompt box the screen shows empty
  holds no message.

## Plan

- [x] Reproduce the defect and find its root cause
- [x] Write the fix spec and the regression test that fails
- [x] The screen's prompt box is read: shown empty only as the real Claude Code draws it
- [x] A message is typed when the box is seen empty, and its hold, mark and flag end
- [x] The pull-request watch still tries a task flagged only for a held message
- [x] The documents and the decision say when a message waits
- [ ] The independent review's findings are answered
- [ ] The full gate passes on the final code and the spec carries the evidence

## Acceptance criteria

- AC1 (EBX-001, EBX-003, EBX-004). With real processes and a fake agent that draws the prompt
  box: a person presses a key that leaves nothing in the box. Within the minute `task_message`
  is refused as before. Two minutes after the key it is typed and the agent reads it as a
  prompt of its own; the draft's mark is gone from the run folder. Fails on dev: refused with
  the draft reason.
- AC2 (EBX-001; the screenshot). The same key, eleven minutes old, and a message of Yulius's
  own (the MCP watch's `/mcp reconnect yulius`): the command is typed at the first pass, the
  task is not flagged and its terminal gets no line. Fails on dev: nothing is typed, the task
  reads Needs your action.
- AC3 (EBX-006, EBX-004). With the same agent: text typed into the box and left there holds
  the command eleven minutes later, the task is flagged and its terminal says so (SPEC-480
  AC2 with a box on screen). The person clears the text and sends nothing: within the minute
  the command still waits; after it the next pass types the command, which the agent reads
  alone, and the flag is gone.
- AC4 (EBX-002; pure rule). `prompt_box::shown_empty` over screens drawn as the probes saw
  them: true for the empty box (focused, unfocused, narrow, help below), the suggestion
  (focused and not) and the queue hint; false for a draft (cursor behind it, cursor at its
  start, one character under the cursor, a pasted block, a second row, wrapped), shell mode,
  the history search, the rewind list, a permission dialog, the transcript view, the agents
  view, the same box on the main screen, a box without its lower rule, a cursor elsewhere.
- AC5 (EBX-002). A hub rebuilt from a snapshot of a screen with an empty box, and of one with
  a suggestion, still reads it as shown empty; one with a draft does not.
- AC6 (EBX-005). With real processes: text left in the box holds a pull request's news of a
  red run; eleven minutes later the task is flagged and a second pass leaves it so. The person
  clears the text and sends nothing: past the minute the watch types the news, the agent reads
  it alone and the flag is gone. A task flagged without that reason is not typed into, its box
  empty or not. Fails without EBX-005: the news is never typed. Pure rule
  (`held_message_flag_outlived`): only that flag over a box shown empty outlived its reason.
- AC7 (EBX-007). The documents name the rule; the gate's document checks pass.

## Test plan

- `crates/yulius/tests/task_message.rs`:
  `a_key_that_left_nothing_in_the_prompt_box_holds_no_message` (AC1),
  `a_message_of_yulius_is_typed_into_a_box_a_key_left_empty` (AC2),
  `a_draft_the_screen_shows_holds_until_it_is_cleared` (AC3), on SPEC-480's movable clock
  with a `--tui` fake agent. The key that leaves nothing is Backspace in an empty box; the
  draft is cleared with `Ctrl+U`, which the PTY's line discipline erases on screen as Claude
  Code does.
- `crates/yulius-app/src/prompt_box.rs`: unit tests of AC4, each screen written as the bytes
  that draw what the probe rendered (attributes included).
- `crates/yulius-app/src/hub.rs`: AC5.
- `crates/yulius/tests/task_message.rs`:
  `a_flag_of_a_held_message_does_not_stop_the_pull_request_watch` (AC6), with the pull
  request, its head and its red run as `pr_watch.rs` sets them up; the pure rule in
  `crates/yulius-app/src/task_message.rs`
  (`only_a_held_message_flag_over_an_empty_box_outlived_its_reason`).
- The same file: `a_box_seen_empty_ends_the_hold_its_mark_and_its_flag_without_a_prompt_hook`
  (EBX-004), with an agent that runs no hooks, so nothing but the screen ends the hold.
- `crates/yulius-domain/src/intake.rs`: `an_empty_box_ends_a_possible_draft_after_the_minute`
  (EBX-003, EBX-004).
- SPEC-480's tests stay green unchanged: their fake agent draws no box, so nothing is proven
  empty there.
- Mutation checks by hand (Evidence): each clause of EBX-002 removed makes a unit test fail;
  without the minute AC1 fails; without ending the flag AC3 fails.

## Decisions

- **The screen is read only as proof that the box is empty** (ADR-0329). SPEC-480 rejected
  reading the box because a wrong "empty" is its defect. That stays the measure: the reading
  accepts one exact picture, probed with the real tool, and everything else falls back to
  SPEC-480's hold. A Claude Code that draws differently costs the old false flag, not a glued
  prompt.
- **Not a rule per key.** Escape hands queued prompts back and the Up arrow refills the box
  (task #454), so "this key leaves no text" is false for the keys people press most.
- **Dim text counts as nothing typed.** Claude Code's suggestion is on by default and shows
  after most turns: without this the commonest idle screen would still hold messages. Typed
  text was never dim in the probes (focused, unfocused, mid-turn, pasted). Turning the
  suggestions off for Yulius's sessions instead would take a feature from the person's
  terminal to make a check simpler.
- **A one-character draft under the cursor is not read as a suggestion.** It looks like the
  first cell of one; a suggestion has more than one character, so the inverse cell counts
  only when dim text follows.
- **The cursor must be in the box.** A transcript can show a picture of an empty box (this
  repository's own tests print one); Claude Code parks the cursor in the real one, and in a
  dialog or list while that has the keys.
- **The alternate screen is required.** Only the full-screen renderer was probed, and Yulius
  runs no other (`FULL_SCREEN_ENV`).
- **The minute is kept** rather than trusting the screen at once: the hub's screen follows a
  key by the session's redraw, and a person who just typed is still there.
- **Emptied at the judged keystroke, not at "now".** A key that arrives while the screen is
  read is later than the judged one and stays a possible draft.
- **The texts stay.** "Send what you typed in the terminal, or clear it and send any prompt"
  is still right where the screen proves nothing; where it does, clearing alone is enough,
  which the guide says.
- **Limits, not fixed here.** The restart on a newer Claude Code (SPEC-468 `App::has_draft`)
  and the `/compact` typist for a full context keep their own keystroke rules: they err toward
  leaving the session alone and flag nobody. Other Claude Code versions were not probed: where
  they draw the box differently, SPEC-480's hold stays. The person's keys in the 300 ms between
  Yulius's paste and its Enter still join the message (ADR-0178).
