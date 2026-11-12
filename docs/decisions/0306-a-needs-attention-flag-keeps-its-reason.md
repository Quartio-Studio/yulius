# 0306. A needs-attention flag keeps its reason

## Status

Accepted. Specified in [SPEC-472](../../.ai/specs/SPEC-472-hey-i-have-a-bug-in-produt-sanbox-this-h.md);
task #455, issue #1221. Beside [ADR-0111](0111-agents-ask-a-person-with-needs-you.md) and
[ADR-0304](0304-a-request-yulius-opens-for-a-goal-pause-ends-when-the-agent-works-again.md) (the
requests) and [ADR-0268](0268-a-pressed-attention-bar-holds-its-render-until-the-press-ends.md) (the bar).

## Context

A running task whose activity is `needs_attention` reads "Needs your action". The task view's bar
said, for every such task, "The agent needs your attention. Its question is in the terminal
below.", with a Reply that gives the terminal the keyboard. That is true for one way the flag is
raised: Claude Code's own notice (a permission menu, a question).

Yulius raises the same flag itself, with no question on the terminal: the Stop gate gave up after
three blocks; the task's goal was never set; Claude Code still asks whether to trust the folder; a
restart cut the typing of the first message; a full context could not be recovered; a session
could not be ended; the agent asked for a secret that a person allows on the Secrets page, or for
an action in the sandbox browser. Each said why once, in a notice of the task's terminal: a frame
sent to the viewers connected at that moment and shown for six seconds. A secret request said
nothing there, and a browser request told only the pages connected then (a page opened later
reads the tabs only while its Browser panel is open). Two of these run when Yulius starts, with
no viewer connected. Nothing was kept, so a person who opened the task later was pointed at a
question that was not there, and Reply changed nothing on screen (Claude Code hides the
terminal's cursor and draws its own). A person reported exactly that for a task of another
project (task #455); SPEC-213's screenshots show the same bar over a working agent. Sessions
Yulius starts run with permissions bypassed, so Claude Code's own prompts are the rarer cause.

The flag often stands over ordinary work. The agent's work lifts only a flag that a notice raised;
after the gate gave up, a goal session goes on and the flag stays until a clean Stop or a typed
prompt, and a secret request's flag stays until a person answers it.

A flag that a notice raised had its own way to the same sight. The agent's work answers it by
the time it was raised, which the server kept in memory only. An instance update freezes every
agent mid-turn and the new server reattaches the runs: a task flagged before the update stayed
flagged after the person answered and the agent went on, until its turn ended.

## Decision

- A `needs_attention` flag keeps its reason with the task, in `tasks.attention` (JSON): `kind`
  (`terminal`: Claude Code asks in the task's terminal; `yulius`: Yulius flagged the task itself),
  `problem` (what is wrong, one line), `action` (what the person does), `link` (a page of this
  Yulius where they do it) and `at`, the time the flag was raised.
- The flag and its reason are one statement (`TaskStore::flag_attention`). A task that is already
  flagged keeps its flag and its reason: one flag, one reason, the first.
- A reason speaks only for the flag it was written with. It is read while the task is running,
  reads `needs_attention` and its state time (`state_at`) is the reason's `at`
  (`TaskStore::task_attention`). Every statement that changes a task's status or activity stamps
  `state_at`, so none has to clear the reason, and a flag raised later without one shows none.
- The task's API view carries it as `attention` (`{kind, problem, action, link}` or null).
- Every flag Yulius raises itself gives its reason, in the words of its notice; a secret request
  links to the Secrets page and a browser request to its tab. A Claude Code notice gives
  `terminal` with Claude Code's own message. The notices stay for the viewers connected then.
  Two flags give no reason, because each comes with a `needs_you` request that is kept with the
  task and shown first: a goal pause's and a host move's.
- The reason also says whether the session's own work answers the flag (`work_answers`: Claude
  Code's own question; not a worker's prompt or an agent that needs input). The store answers a
  reason with the time its flag was raised, and a run that is watched again takes that time back
  when the server holds none. So the agent's work answers such a flag after a restart of Yulius
  as it does before one. It is no part of the API view.
- The bar shows a `yulius` reason as it shows a request (Needs you, Problem, To do, Open for a
  page) and keeps its sentence for a `terminal` one, with Claude Code's words. A flag with no
  reason reads "Look at its terminal below." and claims no question.
- Reply gives the terminal the keyboard for every flag, and the terminal's notice says so ("The
  terminal has the keyboard: answer there."). It never opens the text field for a flag.

## Alternatives

- **A `needs_you` request for each flag** (as for a goal pause, ADR-0304). A request stays
  "whatever the agent does meanwhile" and needs its own end; a flag already has its ends (a typed
  prompt, a clean Stop, the answer to a secret request). One request per task also means that the
  gate's request would replace the agent's own open request. Rejected: a reason bound to the flag
  needs no new lifecycle.
- **Keep the notice in the hub and replay it to later viewers.** The hub is memory of the running
  server: a restart of Yulius forgets it, and two of the flags are raised at a restart. Rejected.
- **Clear the reason in every statement that changes the activity.** Six statements today, and a
  seventh could forget. Rejected for the read bound to `state_at`, which they all stamp already.
- **The latest reason wins on a flagged task.** A Claude Code question on a task the gate flagged
  would replace the gate's reason, and stay after it was answered, since the gate's flag stands:
  "its question is in the terminal" over no question, the defect itself. Rejected: the first
  reason is the one whose matter is still open.
- **Reply opens the text field for a reason of Yulius's own** (the answer is often a sentence).
  A Claude Code menu may have opened since the reason was written, and typed text would pick its
  options unseen (SPEC-156 RPL-006). Rejected.
- **A focus ring on the terminal instead of a notice.** It would change every terminal of the
  product (shells, the Superagent) for one button. Rejected.

## Consequences

- A person who opens a flagged task at any time reads what it waits for and where to act; after a
  restart of Yulius too.
- A task flagged by Claude Code's question before an instance update reads Working once the
  person answered and the agent goes on. A flag that work does not answer (a worker's prompt)
  still ends only at the turn's end or a typed prompt: nothing tells Yulius that such a dialog
  was answered; its bar says what was asked.
- API clients read `attention` beside `activity`; older clients ignore it.
- Schema version 53 adds `tasks.attention`. A flag raised before the update has no reason and
  reads the neutral sentence until it ends.
- A second matter on a flagged task is not shown: a Claude Code menu that opens while the gate's
  reason shows is seen in the terminal, where Reply puts the keyboard.
- The reason of a flag is not refreshed while the flag stands: the gate's reasons are those of the
  Stop that raised it.
- The bound is the state's time in milliseconds. A reason of an earlier flag would show for a
  later one only if the task left the first flag and was flagged again, by a statement that writes
  no reason, in the millisecond of the first flag; every flag the application raises writes the
  column, a reason or none.
