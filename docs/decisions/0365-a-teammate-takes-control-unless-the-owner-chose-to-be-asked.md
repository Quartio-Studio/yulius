# 0365. A teammate takes control of a task at once, unless its owner chose to be asked

## Status

Accepted. Specified in [SPEC-510](../../.ai/specs/SPEC-510-change-request-control-for-tasks-to-take.md);
task #492. The person's request of 2026-11-19: "change "request control" for tasks to "take
control" - github changes then etc, unless user selects different option". Amends
[0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md): its rule
"control passes only by the owner's consent" now holds only for an owner who chose to be asked.
Keeps 0331's three classes (view, tidy, control), the owner as the only one who steers, the
request flow and the giver's take-back, and [0060](0060-a-task-is-owned-by-one-person.md)'s
hand-over of the GitHub identity.

## Context

Since 0331 someone who wants to steer a teammate's task asks, and the task stays the owner's
until they answer. A request waits without limit, so a task whose owner is busy or away stays
out of reach of the person who would carry it on, though stop, archive and delete were already
open to them. The person asked for the opposite default: the teammate takes control, the task
then acts on GitHub as them, and only a person who wants consent keeps it.

## Decision

- **Take control.** Anyone who may work in the project (write access, an enabled account, not a
  playground guest) takes control of another person's task at once
  (`POST /api/tasks/{tid}/control/take`, `App::take_task_control`). It passes as a hand-over
  does, through `App::pass_task_control`: one conditional update (`seize_control`: only from the
  owner seen, dropping a waiting request), then the worktree's commit identity, the run's
  fingerprints, a restart of a run that holds the previous owner's personal MCP servers, the
  terminal notice and the audit (`task.control_taken`).
- **The previous owner can take it back and is told.** The take records them as the giver
  (`tasks.control_from`), so 0331's Take back returns the task while control has not moved
  since, also when the taker asks first. They get `task.control_taken` wherever they are
  (a notification and a toast that opens the task, and their Telegram chat under Needs you; a
  take-back reaches the chat the same way).
- **The option is the owner's, per person, on the server, off by default.** Settings → Your
  account → "Ask me before taking control of my tasks" (`users.control_ask_first`, schema 67,
  `PUT /api/me/control`, `Me.user.control_ask_first`). With it on, a take of their task is
  refused (409 naming them) and teammates request control as before; the take reads the choice
  in its own update, so a choice saved first always holds. Each task summary carries
  `owner_asks_first`, so every page offers the right action, and a refused take reads the task
  again so the page offers what works now.
- **An absent owner never holds a task.** A take, like a request, passes control at once from
  an owner who is disabled, deleted or without write access, whatever they chose (0060, 0331).
- **A confirmation before taking.** It names the owner and says the task will act on GitHub as
  the taker and that the owner can take it back.
- **Taking is a person's own act.** No agent, Superagent or MCP tool takes control (0331's rule
  for asking and consenting).
- **A playground's guest asks.** A guest tidies only their own tasks (0331) and has no GitHub
  identity to act as: the take refuses them, and their pages and refusals offer Request control.

## Alternatives

- **Keep consent for everyone (0331).** Reversed by the person.
- **An option per task.** Nobody sets it before a teammate wants the task, and an owner who wants
  consent would have to set it on each task.
- **An option per project.** One person's wish to be asked would bind the whole team, or the
  team's choice would override theirs.
- **The taker chooses between taking and asking.** It gives the owner no say, which "unless user
  selects different option" asks for.
- **Drop requests altogether.** An owner who chose to be asked needs them, and an asker may still
  prefer to ask; the API keeps them, the pages offer Take control where it works.
- **No take-back after a take.** The owner who lost a task without consent would have to take it
  again, which they cannot when the taker asks first.

## Consequences

- A person can lose control of their running task to a teammate at any time unless they choose
  to be asked; the confirmation, the notice, the audit and the notification say who, and Take back
  returns it.
- What the task does on GitHub after a take is under the taker's name, as after a hand-over.
- A page that read a task before its owner changed the option offers the old action until the
  task's next update or a refused take, which reads the task again.
- The refusal of steering says "Take control on the task's page", or "Ask them for control" when
  the owner asks first.
