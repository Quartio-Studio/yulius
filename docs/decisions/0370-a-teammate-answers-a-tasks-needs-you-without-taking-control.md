# 0370. A teammate answers a task's Needs you without taking control

## Status

Accepted. Specified in [SPEC-516](../../.ai/specs/SPEC-516-user-should-be-able-to-answer-yulius-s-n.md);
task #498. The person's request of 2026-11-19: "user should be able to answer yulius's "needs
me" even if they are not the owner, without taking control, just small intervention". Amends
[0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md): answering what
the agent asked leaves control; keeps [0365](0365-a-teammate-takes-control-unless-the-owner-chose-to-be-asked.md)'s
take and request flows as they are.

## Context

A task's agent asks a person with `needs_you` when only a person can act or decide. Since 0331
only the owner steers a task, and an answer is steering, so a teammate who knows the answer has
to take control (0365), which moves the task's GitHub identity, commit identity and MCP servers
to them, or request it from an owner who asks first and may be away. Both are far more than
the one answer the agent waits for.

## Decision

- **Answer is its own task action.** `yulius_domain::task_access::TaskAction::Answer` needs write
  access, owner or not; a playground's guest answers only their own tasks. The rule stays in the
  one place every way to a task asks.
- **One labelled answer, typed as a message is.** `POST /api/tasks/{tid}/needs-you/answer`
  (`App::answer_needs_you`) cleans the text as `task_message` does and types it through
  `App::type_into_session` (every guard of a typed message holds: a session not started, a
  person's draft, another typist, a withheld screen), labelled "Answer from <login> to what you
  asked with needs_you", which from a non-owner says they are not the owner, that the task stays
  the owner's and that the agent acts on it only as the answer. The request (and a shared
  episode) ends. Audited `task.needs_you.answered`, never the text.
- **Control does not move.** Owner, giver, waiting request, GitHub token and commit identity stay.
- **Not into the owner's personal reach.** From anyone but the owner, an answer is refused (409
  naming the owner) while the owner's personal agent secrets switched on for the project or
  personal MCP servers reach the run: they act as the owner outside GitHub, which is why
  `task_message` is the owner's. Taking control, whose restart drops them, remains the way.
- **Not into a terminal question.** While the agent asks in its terminal (`needs_attention`) the
  paste would press a menu's keys unseen; the owner (or a taker) answers there.
- **The owner's "ask me first" choice does not stop answers.** It is about taking control; an
  answer moves nothing, and for such an owner it is the only help a teammate gives without them.
- **The task view's Reply** on someone else's task sends to the endpoint; a refused answer keeps
  the field and its text.

## Alternatives

- **Open the terminal to non-owners while a request is open.** Any key, any menu: far more than
  an answer, and the terminal follows its writer's size.
- **A temporary hand-over.** Moves the GitHub identity, which the request rules out.
- **Let any person's Superagent `task_message` a teammate's task.** An agent's act, not the
  person's, and not asked for.
- **Answer into the owner's personal reach with a warning in the label.** A label does not stop
  an agent from using the owner's credentials for another person's words.
- **Notify the owner of every answer.** The request ends as when they answer themselves; the
  terminal, transcript and audit keep who answered. Can follow if owners ask for it.

## Consequences

- A teammate unblocks a task waiting on a person with one reply, and the task keeps acting on
  GitHub as its owner, who can read the answer in its transcript.
- An owner whose personal secrets or MCP servers reach their tasks answers them alone (or loses
  the task to a take, which drops them).
- A new way to act on a task names its class: view, tidy, answer or control.
