# 0309. Yulius types nothing onto a person's possible draft, and tells them after ten minutes

## Status

Accepted. Specified in [SPEC-480](../../.ai/specs/SPEC-480-a-message-yulius-types-lands-on-a-person.md);
task #463, issue #1199. Amends [ADR-0178](0178-the-superagent-messages-tasks-by-queueing-or-typing.md)
("the person's draft counts for a minute") and SPEC-230 TMS-004.

Amended by SPEC-487 (TCT-006, task #469): the task's owner, and only they, may have a message
sent past their own possible draft (`task_message` with `with_draft`): it is typed behind the
text and both are submitted as one prompt, once their last key is more than a minute old.
Yulius's own messages and everyone else's still wait.

## Context

Yulius types messages into running sessions through one function, `App::type_into_session`: a
person's `task_message`, a pull request's news, "Yulius is back; continue", the MCP watch's
texts. It held a message back only while a person had typed in the last minute. A draft left
longer got the paste behind it and the Enter: the person's unfinished text was submitted
together with the message, as one prompt (issue #1199).

Yulius cannot read the prompt box. Since SPEC-468 it knows two times per session: when a person
last typed, and when the box was last empty (the run's start, or the last prompt submitted from
it). A keystroke after the box was emptied may have left text there. It may also have left
none: an answer to a dialog, Escape, a command that submits no prompt. Yulius cannot tell these
apart, so a rule on that knowledge holds messages to a session whose box is in fact empty until
a prompt is submitted from it.

## Decision

- **A person's keystroke since the prompt box was last empty holds every typed message, however
  old it is.** `type_into_session` refuses with `Conflict` before it writes anything
  (`policy::box_held`). Only a person's keystrokes count, every one of them. The minute after
  any keystroke stays as well.
- **Yulius never types anyway.** No bound after which the message goes in: that would be the
  same defect with a longer window, and a prompt that was submitted cannot be taken back.
- **The one who can act is told.** A sender (`task_message`) gets the reason in the answer, at
  once. For a message of Yulius's own, which waits in its watch, the task is flagged Needs your
  action and its terminal says so once the person's last keystroke is ten minutes old
  (`policy::DRAFT_TOLD_MS`), once per keystroke. Sending what they typed, or any prompt, ends
  the hold; the watch types at its next pass.
- **Ten minutes, counted from the person's last key.** Long enough that someone still writing
  is not flagged, short enough that a pull request's news is not silently late for long. It
  needs no state per waiting message. A message Yulius gives up (the wake after an outage ends
  ten minutes after the start) tells the person at its last try, whatever the key's age.
- **The flag ends with the hold.** A prompt that starts a turn sets the task Working, which
  ends every flag. One that starts none (`/compact`) only empties the box, so the hook that
  records the emptied box also ends this flag, and only this one.
- **The draft's mark is kept in the run folder.** The keystroke that begins a possible draft
  writes `person-typed` (its time) into the run's folder, once; a prompt submitted from the box
  removes it. A restarted server reads it when it reattaches the run, so the wake after an
  outage, which is typed only after a restart, does not land on a draft typed before it. A
  draft the server knows from the mark alone stays one until a prompt follows a key it saw: a
  prompt held across the restart is asked about again, reads as a new submit, and may have a
  draft behind it.

## Alternatives

- **Keep keystrokes in memory only.** As SPEC-468 does for restarts on a newer Claude Code. But
  "Yulius is back; continue" is typed only after a restart of the server, when memory is
  empty: the issue's defect would stay for the typist it names. Rejected (review, round 1).
- **Wait some minutes, then type anyway.** Bounded delivery to a session without a draft, but a
  real draft older than the bound is still submitted with the message. Rejected: it is what the
  issue reports.
- **Count only keys that add text** (not Enter, Escape, arrows). The Up arrow takes prompts
  from Claude Code's queue back into the box (seen with 2.1.322 by task #454), and each further
  key would need its own proof for every Claude Code version. Rejected.
- **Do not count keys typed while the session shows its own dialog.** Yulius knows such a
  question only until the agent works again. Whether a person who declines it is back in the
  prompt box before that was not proven either way, and a wrong guess is the defect. Rejected.
- **Read the prompt box from the screen.** The box's frame and what is drawn in it without
  being typed differ between Claude Code versions; the issue's verification judged it fragile,
  and a wrong "empty" is the defect. Rejected.
- **Stash the draft with a key of Claude Code's own, type, and let it come back.** Not proven
  against the real tool, and it would depend on a key binding people can change and on the
  version. Rejected.
- **Deliver through a hook instead of the prompt box.** An idle session runs no hook
  (ADR-0178). Rejected.
- **Use `App::has_draft`.** It also counts Yulius's own typed message until its submit hook,
  right for a restart and wrong here: a hook that never came would hold every later message.
  Rejected; the rule reads the person's keystroke alone.

## Consequences

- A message that goes through `type_into_session` is not typed behind text a person typed
  into the session's terminal and did not send, at any age, for keystrokes Yulius saw (those
  of a viewer of the task's terminal, also before a restart of the server). The limits below
  and Yulius's other typists (the goal typist, the `/compact` typist for a full context) are
  outside this decision.
- A session a person typed into without leaving a draft (an answered dialog, Escape, an arrow
  key) gets no typed message until a prompt is submitted from its box. Its person is told
  within ten minutes of their last key plus one pass of the watch, and one prompt releases it.
  This is the price of not guessing. How often such keys are typed and left was not measured.
- The telling is a `needs_attention` flag, and a flagged task is not idle: until its person
  sends a prompt, a finished task that is flagged does not count as done where "done" asks for
  an idle task (`task_is_done`: chained tasks, supertasks). As read in the code, not run. It
  is the same for every flag Yulius raises.
- After the wake is given up the flag still says a message waits; the person's prompt, which
  the flag asks for, is what continues the session.
- The pull-request watch types nothing into a flagged task anyway (a person must act first),
  so the flag and the hold end together, at the person's prompt.
- The outage wake still gives up ten minutes after the start: a session with a draft continues
  with its person's prompt.
- After a restart the server knows a draft from its mark, with the time of the key that began
  it (later keys of the same draft are not written): the person may be told earlier than ten
  minutes after their last key. `App::has_draft` (SPEC-468) reads the same time, so a session
  with a draft from before a restart of the server is not restarted on a newer Claude Code
  either. A mark that could not be written is logged and tried again at the next key.
- A session whose mark outlived its draft across a restart (a prompt held over the restart
  with nothing typed behind it) holds messages although its box is empty, until a prompt
  follows a key typed after the restart; its person is told like any other.
- A prompt Claude Code scheduled for itself ends the hold early until issue #1225 is fixed.
- Reversible: the rule is `policy::box_held` and one constant; a later, proven way to know the
  box is empty only has to move `box_emptied_ms`.
