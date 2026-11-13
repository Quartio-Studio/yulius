---
yulius-task: e68c4e9a-3bfb-42b0-bf5d-e6fa5aeec9dc
status: done
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
  the `vt100` crate the hub uses; 537 screens over 62 runs, the author's and the independent
  reviewer's, in `$HOME/.cache/yulius-task-470` and `$HOME/.cache/yulius-review-470`). The
  prompt box is one row `❯` between two rows of `─` across the whole width, on the alternate
  screen, and the terminal's cursor is parked at the box's first cell (column 2 of that row)
  while the box is empty. What was seen:
  - empty box (fresh, mid-turn, after a turn, after Escape idle and mid-turn, after a declined
    permission dialog, after `Ctrl+U`, with the terminal unfocused, from 120 down to 20 columns
    wide, with the shortcut help open below it, in vim mode, in all six built-in themes): `❯`
    alone, cursor at column 2, rules not dim;
  - a draft: its text after `❯` in normal intensity, the cursor behind it; with the cursor
    moved to the start (`Ctrl+A`) the cursor is at column 2 and the text stays in normal
    intensity, focused or not, idle or mid-turn, in every theme; a one-character draft under
    the cursor is one inverse cell; a pasted block is `[Pasted text #1 +39 lines]` and an
    image `[Image #1]` in normal intensity; a draft of several rows (a first row left empty,
    or wrapped) has more than one row between the rules;
  - what Claude Code draws in an empty box by itself: its prompt suggestion (on by default)
    and "Press up to edit queued messages", both **dim** with the cursor at column 2 (the
    first character inverse under the caret while focused, dim when not). A paste into a box
    that shows a suggestion replaces it: the prompt hook got the pasted message alone;
  - the row above the upper rule is Claude Code's hint row: blank; or a notice that lasts
    seconds ("Ctrl+Y to paste deleted text" was gone 21 s after the deletion; a dismissed
    dialog's line; the usage-limit warning, gone 30 s after its turn); or "N% context used"
    for as long as the session's context is nearly full (88% at about 159k of 200k tokens,
    none at 139k; idle, mid-turn and 90 s after a further turn); or `› stashed` for as long as
    a draft is put aside with the stash key (`Ctrl+S`; seen 67 s later, down to 10 columns and
    4 rows, and behind the context notice as "88% context used · › stashed"). A stashed draft
    leaves the box empty and comes back into it, with no keystroke, after the next submit or
    local command (review run r1: a first pasted message went alone, a second was submitted
    as "my precious draftMessage from Yulius two");
  - the agents view (Left arrow from an empty box) has a box of its own, which starts a new
    background session with what is entered into it (review run r13: a second `SessionStart`).
    Its rules are dim in every theme, the conversation's never are; its placeholder is grey
    in five themes and dim, like a suggestion, under `dark-ansi`. A session opened from that
    view shows an ordinary box whose upper rule ends in its name ("─ stub title ─"): a paste
    there went to that session, not the task's (review runs q2, q10). The task's own session
    carries such a name after `/rename`, and after a trip through the agents view;
  - other states have no such box at the cursor: shell mode (`!` instead of `❯`), the history
    search (a labelled upper rule and the entry's text), the rewind list, a permission dialog,
    the transcript view, the 17 slash-command dialogs the review opened, a footer pill that
    has the keys (the cursor leaves the box).
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
  below consist of `─` across the whole width and are not dim (the agents view's are; a rule
  that carries a name is another session's, or this one renamed); the row above the upper
  rule is blank or holds, whole, only the notice "N% context used" (any other hint there,
  `› stashed` above all, is no proof); and from column 2 on the box's row holds nothing, or
  only dim text (Claude Code's own suggestion or hint): every non-blank cell is dim, except
  that the cell under the cursor may be inverse when a dim cell follows it. Anything else is
  not proof, and the hold stays as SPEC-480 has it: text in normal intensity, a second row in
  the box, another prompt character, a labelled or dim rule, a hint above the box, a dialog
  or another view, the main screen, a withheld screen, a Claude Code that draws differently.
- EBX-003 (the minute stays). A keystroke of the last minute holds a message as before
  (SPEC-480 DRF-002), whatever the screen shows: the person is at the keyboard, and the screen
  may be behind their key.
- EBX-004 (the hold ends). A box seen empty counts as emptied at the keystroke that was judged:
  the draft's mark leaves the run folder and the flag of a held message ends (SPEC-480
  DRF-007, DRF-009), as after a prompt submitted from the box. A key typed after that is a
  possible draft of its own, also one typed while the mark and the flag are ended: the hold
  is judged again before anything is pasted. This also holds for a draft known from its mark
  alone (after a restart of the server): the screen is what the session shows now.
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
- [x] The independent review's findings are answered
- [x] The full gate passes on the final code and the spec carries the evidence

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
  them: true for the empty box (focused, unfocused, help below, text deleted), the suggestion
  (focused and not) and the queue hint; false for a draft (cursor behind it, cursor at its
  start, one character under the cursor, a character behind a blank or behind the cursor's, a
  pasted block, a second row, wrapped), a hint above the box (`› stashed`, deleted text, the
  context notice with a stash behind it, cut short or reworded), the agents view under
  `dark-ansi` (dim rules, one or both, or in part), a rule that ends in a session's name,
  shell mode, the history search, the rewind list, a permission dialog, the transcript view,
  the agents view's grey placeholder, the same box on the main screen, at the screen's top,
  without its lower rule, a cursor elsewhere; true again with the context notice alone above
  the box, and for a box whose hint row is the screen's first. The same reader over all 537
  captured screens of the real tool (`verdicts.py`): 213 read empty, each with `❯` alone or
  Claude Code's own dim text in the box and a blank hint row (192) or the context notice
  (21); no screen with typed text, a stash, the agents view or a named rule reads empty.
- AC5 (EBX-002, EBX-004). A hub rebuilt from a snapshot of a screen with an empty box, and of
  one with a suggestion, still reads it as shown empty; one with a draft does not. With real
  processes and a restarted server: a key that left nothing in the box before the restart
  holds a message within the minute and none after it; the mark is gone.
- AC6 (EBX-005). With real processes: text left in the box holds a pull request's news of a
  red run; eleven minutes later the task is flagged and a second pass leaves it so. The person
  clears the text and sends nothing: past the minute the watch types the news, the agent reads
  it alone and the flag is gone. A task flagged without that reason is not typed into, its box
  empty or not. Fails without EBX-005: the news is never typed. Pure rule
  (`held_message_flag_outlived`): only that flag over a box shown empty outlived its reason.
- AC7 (EBX-007). The documents name the rule; the gate's document checks pass.
- AC8 (EBX-004). A key recorded between the screen's reading and the paste (a pause point)
  gets the message refused; nothing is typed. With an agent that runs no hooks, a box seen
  empty alone ends the flag and removes the mark.

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
- `crates/yulius-app/src/hub.rs`: AC5 (`a_snapshot_keeps_the_prompt_box_readable`);
  `crates/yulius/tests/task_message.rs`:
  `a_key_before_a_restart_that_left_nothing_holds_no_message` (AC5, through the process
  host's own snapshot) and `a_key_typed_while_the_empty_box_is_taken_in_holds_the_message`
  (AC8, at the pause point `race::Point::BoxSeenEmpty`).
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
  empty there. The fake agent's `--tui` keeps the row above its box blank, as Claude Code's
  hint row is.
- Mutation checks by hand (Evidence): each clause of EBX-002 removed makes a unit test fail;
  without the minute AC1 fails; without ending the mark and the flag the hookless test fails.

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
- **A hint above the box is no proof, unless it is the context notice alone** (review rounds
  1 and 2). The stash is the reason: a stashed draft is not in the box, but it returns there
  by itself after the next submit, so the message after the one typed now would be glued to
  it. The rule does not look for the word "stashed": a renamed hint still refuses. One notice
  is let through by its exact words, "N% context used", because it stays for as long as the
  context is nearly full and would bring the false flag back for just the long sessions; a
  stash shows behind it in the same row, so the row must hold nothing else. Other notices
  there last seconds and cost one try of a watch; one not seen with the real tool is no
  proof.
- **A named rule is no proof** (review round 2). The name on the upper rule is all that tells
  another session's box from the task's. A session a person renamed (`/rename`), or one that
  was in the agents view, therefore keeps SPEC-480's hold and flag: not telling the two
  apart is the worse error.
- **Dim rules are not the prompt box's** (review round 1). The agents view's box takes a paste
  and Enter as a new background session, and under `dark-ansi` its placeholder is dim. Its
  rules are dim in all six themes and the conversation's in none, over every captured screen.
- **The alternate screen is required.** Only the full-screen renderer was probed, and Yulius
  runs no other (`FULL_SCREEN_ENV`).
- **The minute is kept** rather than trusting the screen at once: the hub's screen follows a
  key by the session's redraw, and a person who just typed is still there.
- **Emptied at the judged keystroke, not at "now".** A key that arrives while the screen is
  read is later than the judged one and stays a possible draft.
- **The texts stay.** "Send what you typed in the terminal, or clear it and send any prompt"
  is still right where the screen proves nothing; where it does, clearing alone is enough,
  which the guide says.
- **Limits, not fixed here.** Text Claude Code puts back into the box with no keystroke after
  a person's own submit (a stash restored) is unknown to SPEC-480's rule and typed behind:
  pre-existing, issue #1264. The restart on a newer Claude Code (`App::has_draft`, SPEC-468)
  and the `/compact` typist for a full context ask the keystroke rule alone and never the
  screen; `has_draft` reads `box_emptied_ms`, so once a message was typed into a box seen
  empty, that key is no draft for the restart either. Other Claude Code versions were not
  probed: where they draw the box differently, SPEC-480's hold stays. The person's keys in
  the 300 ms between Yulius's paste and its Enter still join the message (ADR-0178). Whether
  the browser terminal passes `Ctrl+S` on was not checked; the rule does not depend on it.

## Evidence

Run in this task's worktree on 2026-11-13; logs under the ignored `target/evidence/`, the
probe in `$HOME/.cache/yulius-task-470`, the review's in `$HOME/.cache/yulius-review-470`.

- **Reproduced before the fix** (commit d638f48cc: the reader and the tests, the rule as on
  `dev` 607121220): `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (…three
  tests…)'`: 0 passed, 3 failed. `a_key_that_left_nothing_in_the_prompt_box_holds_no_message`:
  `Conflict("a person typed into task #1's terminal and sent no prompt since, so their text
  may still be in its prompt box and nothing is typed behind it: …")`.
  `a_message_of_yulius_is_typed_into_a_box_a_key_left_empty`: nothing typed, attention
  `"problem":"Yulius has a message for this session and does not type it: someone typed in
  the terminal and sent no prompt since, so their text may still be in the prompt box."`, the
  reported screen. `a_draft_the_screen_shows_holds_until_it_is_cleared` failed at its last
  pass (a cleared box released nothing). Reported as issue #1260.
- **The pull-request watch reproduced** (commit aab3762d4, with the fix of the hold but not of
  the watch): `a_flag_of_a_held_message_does_not_stop_the_pull_request_watch` failed with "the
  news was not typed into the flagged task".
- **The real tool** (Claude Code 2.1.323 against a local stub of the model API, no account):
  `verdicts.py` runs `prompt_box::shown_empty` (the file of this branch, included by path)
  over every captured screen: 537 screens over 62 runs (the author's 15 runs, the reviewer's
  47). 213 read empty: each holds `❯` alone (144) or Claude Code's own dim text (44 the
  suggestion, 14 the queue hint, 11 its `Try "…"` placeholders), under a blank hint row (192)
  or the context notice (21). No screen with typed text, a stash, the agents view or a named
  rule reads empty. In all six built-in themes (`s8.json`, 12 states each): empty, suggestion
  (focused and not), cleared, mid-turn and queue hint read empty; draft, draft with the cursor
  home, stash and agents view do not. A paste over a suggestion and Enter: the prompt hook
  got `Message from Yulius:\n\nCI failed` alone (run s4).
- **After the fix**, on the merged head e89826b1c:
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & (test(/^task_message::/) |
  test(/^pr_watch::/))'`: 29 passed (the 21 earlier tests of `task_message::` unchanged, 7
  new, and `pr_watch::`); the reader's unit tests, the hub's and the two rules':
  `cargo test -p yulius-app --lib prompt_box` 7 passed, `a_snapshot_keeps` 1,
  `only_a_held_message_flag` 1, `cargo test -p yulius-domain an_empty_box_ends` 1.
- **Mutation checks by hand**, 31, each restored and touched afterwards
  (`target/evidence/mutate*.py`, logs `mutations-*.log`): 30 fail a test; one survived and
  showed a redundant guard, which was removed.
  - the reader, 12: no alternate screen, no cursor column, no prompt character, no blank
    behind it, no upper rule, no lower rule, a shorter rule, any text behind the first cell,
    an inverse first cell without dim text behind it, dim text taken for typed, the first
    cell not looked at: each fails a unit test; "a box in the screen's last row" survived
    (the rule check already refuses it; the guard went);
  - the fix, 6: no minute (AC1 and the domain test fail); a box seen empty ends neither mark
    nor flag, and does not count as emptied (the hookless test fails for both); the screen
    not asked (seven tests fail, SPEC-480's among them); a flag outlived its reason whatever
    the screen shows (the pure rule's test); no flag ever did (the pull-request watch test);
  - review round 1, 5: no hint row, a dim rule accepted, the hold not judged again, half the
    hint row, the cell behind the first skipped (the reviewer's survivor): each fails;
  - review round 2, 8: four ways of reading the context notice loosely, no notice accepted,
    and the reviewer's three survivors (the hint row's right half, dim at a rule's first cell
    only, two rows asked above the rule): each fails.
- **The gate** (`scripts/dev ci`): at e2869044c (before the review) exit 1: 4256 tests, 4237
  passed, 19 failed, all "timed out waiting for …" within seconds of each other in
  `usage_credits::`, `task_done::`, `trusted_proxy::` and three of `task_message::`, at a
  load average of 98 on the shared 64-core host (the review was building beside it); the
  same 19 alone at once after: 19 passed. At 68ae83200 (review round 1's fixes): exit 0, 4260
  tests passed, 16 skipped.
  At e89826b1c (review round 2's fixes, with `origin/dev` e662ef766 merged in without a
  conflict and the decisions index put back in order): exit 0, 4295 tests passed, 17 skipped.
  After that only this section, the status and the Plan's ticks changed.
- **Security scan**: `yulius security-scan` printed "Security scan disabled: Vidoc is turned
  off in Yulius, for this project or the whole instance; nothing was scanned." Unrun, not
  passed.
- **Independent review, round 1** (a fresh session, head e2869044c, skill `om-code-review`,
  with its own runs of the real tool): "request changes". Its findings and what was done:
  - own code, major: "a stashed draft reads as 'empty' and is glued to Yulius' second
    message." Fixed test-first: a hint above the box is no proof (EBX-002).
  - own code, major: "under theme dark-ansi the agents view reads as an empty prompt box,
    and Yulius' message creates a new background session." Fixed test-first: dim rules are
    refused; probed in all six themes.
  - own code, minor: "box_seen_empty awaits store I/O … between the screen read and the
    paste; `held = None` is unconditional". The hold is judged again; a test at a pause
    point (AC8).
  - own code, minor: one mutant of the reader's tests survived. Two cases added.
  - own code, minor: three statements in the spec and ADR-0329 went further than the code or
    the runs (`has_draft`, "every other state", an Evidence section cited before it existed).
    Rewritten to what was run; this section.
  - own code, nits: a long doc line; "grey" for the dim attribute; the hub's snapshot test
    "proves the hub's `viewer_snapshot`, a twin of the process host's". Fixed; a test across
    a real restart added (AC5).
  - pre-existing, major: "text Claude Code puts into the box without a keystroke is unknown
    to box_held" (a stash restored by the person's own prompt). Issue #1264, with the nit
    about `draft_ended`'s two steps.
- **Independent review, round 2** (the same reviewer, head 68ae83200, the delta): "request
  changes (one major in the delta's own code)"; round 1's findings checked as fixed. Its
  findings and what was done:
  - own code, major: "'N% context used' stays in the hint row for as long as the session's
    context is high, so the reported false flag (#1260) is back for such sessions … Fails
    safe (nothing glued)." Fixed test-first: that notice alone, whole, is as good as a blank
    row; with a stash behind it the row is no proof.
  - own code, minor: "a labelled upper rule is the only thing that tells another session,
    and it is neither tested as such nor documented." A unit test with a name at the rule's
    end; the spec, ADR-0329 and the guide say that a named rule is no proof and that a
    renamed session keeps the old hold.
  - own code, nits: three surviving mutants (tests added, all three fail now); a one-second
    sleep as a negative wait in the pause-point test (removed).
  - outside the change, not checked in Yulius by the reviewer: "a trip through the agents
    view re-homes the conversation" (new `SessionStart` hooks, another session id). Read in
    the code afterwards: `hook_session_start` takes a new session id of the run's own Claude
    Code into the attempt and the lineage (SPEC-281), so Yulius follows it by design; not
    run with the real tool, and no issue was filed for it.
  A third look, at this section's commit, follows it; its verdict is in the pull request.
- **Not tested here.** Yulius end to end with the real Claude Code: the reader was run over
  the real tool's screens, and the hold's logic with the fake agent, whose `--tui` draws the
  same box. Other Claude Code versions than 2.1.323; a model with a 1M context (the context
  notice's threshold was seen on a 200k model); a task list shown mid-turn; whether the
  browser terminal passes `Ctrl+S` on. That the reviewer's reading of a re-homed session is
  harmless in Yulius was read in the code, not run. The web UI is unchanged, so the
  end-to-end suite was not run.
