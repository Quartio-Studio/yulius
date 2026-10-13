# 0050. Only keystrokes count as typing; a typed first message stays pending until submitted

## Status

Accepted. Specified in
[SPEC-068](../../.ai/specs/SPEC-068-fix-252-266-only-real-typing-counts-as-t.md); amends
[0026](0026-attached-images-are-pasted.md) (its two #266 gaps) and SPEC-025's known limit on
drafts (#252).

## Context

The web terminal forwards everything xterm.js emits. Besides keys, that is focus reports (Claude
Code enables focus reporting), mouse reports and xterm's answers to the program's queries. Every
such frame counted as typing: it claimed control of the terminal size and moved the "a person
typed" time that prompt attribution (TSK-013) and the goal typing read. So Yulius could not skip
the goal when a person typed: a click would have left a task without a goal, and a real draft got
`/goal` appended and sent.

A first message with images is typed by the server (ADR-0026). Nothing recorded that it was
pending: a restart during the typing left the session at a half-filled prompt reading Working,
and a first message held for capacity was carried to another account as the hook's text, without
its images.

## Decision

- The server classifies each input frame: it is typing when it holds a keystroke (text, a control
  key, an Escape key sequence or a bracketed paste). Focus reports, SGR and X10 mouse reports and
  terminal answers (device attributes, cursor position, mode and status reports, OSC and DCS
  strings) are sent to the session but neither record typing nor claim control.
- A goal task whose terminal a person typed into after the launch gets no `/goal`: the draft is
  left alone, the task needs attention and its terminal says why.
- A run whose first message Yulius types has an `opening-pending` file in its run directory,
  written before the spawn and removed by the session's first `UserPromptSubmit` let through.
  - After a restart, a reattached run still pending (and not held for capacity) is flagged
    NeedsAttention with a notice; it is not typed again.
  - A prompt held for capacity while the first message is pending resumes on another account as
    a fresh session, which types the task's own opening with its images.

## Alternatives

- Filter the reports in the browser: every client would need the same filter, and the browser
  does not see all the answers xterm.js sends by itself. The server decides what typing means.
- Wait for the draft to be submitted, then type the goal: a draft may never be sent.
- Type the first message again after a restart: how much of it reached the prompt box is unknown;
  typing over a half-filled box duplicates images or mangles the request.
- Keep the pending state in memory or in the store: memory is lost by the restart it must
  survive; a store column needs a migration for state that lives exactly as long as the run's
  directory.
- Match the held prompt's text against the request: Claude Code's text for pasted images is not a
  contract.

## Consequences

- Clicking into or out of the terminal no longer moves control or counts as typing.
- A Shift-F3 key (`ESC [ 1 ; 2 R`) reads as a cursor position answer: not typing. Claude Code has
  no use for it.
- A person who types during a goal task's start must type `/goal` themselves (the notice says so).
- A restart during the first message's typing needs a person to finish that message, and to paste
  its images again if they are missing.
- A first message sent while Yulius is down (the hook client fails open) stays pending, so the
  restart flags that task although it runs: a rare false alarm a look at the terminal clears.
- The held session is replaced by a fresh one: it had no turn yet, so nothing is lost.
