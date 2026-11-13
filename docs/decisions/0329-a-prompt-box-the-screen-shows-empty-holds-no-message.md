# 0329. A prompt box the screen shows empty holds no message

## Status

Accepted. Specified in [SPEC-488](../../.ai/specs/SPEC-488-fix.md); task #470, issue #1260.
Amends [ADR-0309](0309-yulius-types-nothing-onto-a-persons-possible-draft.md) ("Read the prompt
box from the screen. Rejected.") and SPEC-480 DRF-001, DRF-006 and DRF-007.

## Context

ADR-0309 holds every message Yulius types into a session while a person typed after its prompt
box was last known empty, and knew the box empty only from the run's start or a prompt
submitted from it. Every key counts, also the many that leave no text: Escape, an arrow or page
key, an answered dialog, a command without a prompt hook, text typed and deleted. After such a
key no pull request's news, no "Yulius is back; continue" and no `/mcp reconnect yulius`
reached the session until a person sent a prompt, and ten minutes later the task read Needs
your action over an empty prompt box. ADR-0309 called that "the price of not guessing". A
person reported it the day after (issue #1260): an autonomous task's person has no reason to
send a prompt, so the price was a finished task that asked for one.

ADR-0309 rejected reading the box from the screen as fragile, without having looked at the
real tool. Task #470 looked: Claude Code 2.1.323, full screen as Yulius runs it, in a PTY
against a local stub of the model API, rendered with the `vt100` parser the hub uses, 51
screens over seven runs (SPEC-488, Context and Evidence). An empty box is one row `❯` between
two rules across the width, on the alternate screen, with the cursor parked at the box's first
cell. Typed text is drawn in normal intensity; what Claude Code draws into an empty box by
itself (its prompt suggestion, on by default, and the hint while prompts are queued) is dim,
with the cursor still at the first cell, and a paste replaces it. Every other state (shell
mode, the history search, a dialog, a list, the transcript and agents views) has no such box at
the cursor.

## Decision

- **The screen is read as proof that the box is empty, and as nothing else.**
  `prompt_box::shown_empty` accepts exactly that picture: the alternate screen; the cursor in
  column 2 of a row that starts with `❯` and a blank; rules across the whole width directly
  above and below; from column 2 on nothing, or only dim text (the cell under the cursor may
  be inverse when dim text follows). Anything else proves nothing and ADR-0309's hold stays.
- **A box shown empty ends the hold of a possible draft once the minute after the key has
  passed** (`BoxHeld::ends_at_an_empty_box`, in `App::type_into_session`). The minute stays
  whatever the screen shows: the person is at the keyboard and the screen may be behind.
- **The box then counts as emptied at the judged keystroke**, as after a submitted prompt: the
  draft's mark leaves the run folder and the flag of a held message ends (`App::draft_ended`).
  A later key is a possible draft of its own. A draft known from its mark alone (after a
  restart) is judged by the screen as well.
- **A flag that only says a message waits, over a box shown empty, does not stop the
  pull-request watch** (`held_message_flag_outlived`). The watch types nothing into a task
  whose person must act; this flag asks nothing its next try could not settle, so the try runs,
  types the news and ends the flag. Every other flag, and this one over any other screen,
  stops the watch as before.

## Alternatives

- **Leave it** (ADR-0309's consequence). A false Needs your action on tasks that need nobody,
  and their pull request's news never delivered. Rejected: it is the reported defect.
- **A rule per key** (ADR-0309 rejected it). Still wrong: Escape hands queued prompts back and
  the Up arrow refills the box.
- **Read any text after `❯` as a draft, with no rule for dim text.** Simpler, but Claude Code's
  suggestion shows after most turns, so the commonest idle screen would still hold messages.
  Rejected.
- **Turn Claude Code's prompt suggestions off in Yulius's sessions** so an empty box is always
  blank. It takes a feature from the person's terminal to simplify a check. Rejected.
- **Trust the screen at once, without the minute.** The hub's screen follows a key by the
  session's redraw, and a person who just typed is still there. Rejected.
- **Let the watch try every task flagged for a held message, whatever the screen shows.**
  `type_into_session` would refuse where the hold stands, but a flag left behind by a failed
  store write, over a dialog, would get a paste. Rejected: the screen is asked first.

## Consequences

- A keystroke that left nothing in the prompt box holds a typed message for a minute, as any
  keystroke does, and flags nobody. Text in the box holds every message as ADR-0309 decided;
  clearing it releases them without a prompt.
- A task flagged before this change over an empty box is released at the watch's next pass.
- The reading is tied to what Claude Code 2.1.323 draws full screen. A version that draws the
  box differently is not recognised, and ADR-0309's hold and flag come back for it: the
  failure is the old false flag, not a prompt glued to a draft. A version that drew typed text
  dim with the cursor at the box's start would be read as empty; none of the probed states
  does (typed text focused, unfocused, mid-turn, pasted, with the cursor moved home).
- A screen that shows the box empty while keys go elsewhere was not seen: the views that take
  the keys (dialogs, lists, the agents view) draw no empty box at the cursor.
- The restart on a newer Claude Code (`App::has_draft`, SPEC-468) and the `/compact` typist
  keep their keystroke rules; they flag nobody.
- The probe is kept outside the repository (`$HOME/.cache/yulius-task-470`: driver, stub,
  renderer, and a script that runs the reader over every captured screen); it needs no account
  and is to be run again when the box's picture is in doubt.
