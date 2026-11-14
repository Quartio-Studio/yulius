# 0331. Anyone who may work in a project tidies its tasks; control passes only by the owner's consent, which they can withdraw

## Status

Accepted. Specified in [SPEC-490](../../.ai/specs/SPEC-490-users-may-stop-archive-and-delete-other.md);
task #473. The person's decision of 2026-11-14: "users should be able to delete stop and
archive tasks of other users, but not take control without the consent". Amends
[0060](0060-a-task-is-owned-by-one-person.md): its rule "only the owner changes a task" becomes
"only the owner steers a task", and its rejected alternative "leave stop, archive and delete
open to any writer" is now the decision. Keeps 0060's ownership, credentials and hand-over, and
[0145](0145-someone-else-s-task-is-watched-as-its-live-transcript.md)'s watching.
Delivered in two pull requests: the rule, its three classes and the web UI first; the owner
hearing of a request wherever they are and the giver taking a task back second.

## Context

ADR-0060 gave every task one owner, whose GitHub token it acts with, and refused every change
of a task to anyone else, stop, archive and delete included: "they are the owner's work too".
In practice a task of a teammate who is away keeps its session, a subscription seat and its
place in everyone's lists, and the only way to end it was to ask for control: to take over the
teammate's work and have it act on GitHub under one's own name, for a task one only wants
gone.

The owner check was written where each path needed it: in `App::load_task`, and again as a
comparison of the owner in the terminal, the shells, the editor's files, the queue order and
the bulk tool. Consent existed (ask, hand over, decline, withdraw the request; an owner who can
no longer act loses the task to the first request), but the owner saw a request only on the
task's own page, and a hand-over could not be undone by the person who gave it.

## Decision

- **Three things a person does to a task, one rule.** `yulius_domain::task_access::TaskAction`
  is `View`, `Tidy` or `Control`, decided from the person's access to the project and whether
  they own the task: view needs read access, tidy needs write access, control needs write
  access and ownership. `App::load_task` and `App::require_control` apply it, and nothing else
  in the application compares a task's owner. A sandbox-level action above `View` given to
  `load_task` means `Control`, so a new way to change a task is the owner's unless it names
  `Tidy`.
- **Tidy is stop, close-out, archive, restore and delete.** Stop includes cancelling a task
  that waits to start. Anyone who may work in the project does them to any of its tasks, by
  every way in: the pages, the API, the Superagent, the remote MCP and a task's agent, each
  for the person it acts for. They end a task or put it away; none acts on GitHub.
- **Everything else is control**, the owner's alone, with no exception for administrators:
  typing into the session, messages, answers to what the task asks, held prompts, continue and
  restart, start and schedule, pause and resume, the subscription, the title and the request,
  the queue's order, attachments, the pull request and its merge, files and shells in its
  worktree, and the answer to a request for control. Three ways in that asked no owner now ask
  the rule: a Superagent's `task_message` to a running task (SPEC-230 allowed it unless the
  task held personal secrets), a restore of attachments into a task, and Start onboarding.
- **An onboarding task is its starter's.** A sandbox whose onboarding nobody started has no
  task; it is created for the person who starts onboarding and acts on GitHub as them. One
  that exists is started by its owner, like any task.
- **What is the project's is not a task's.** The sandbox's one browser, CI runs and the
  repository's cleanup are shared by everyone who may work in the project; using them steers
  no session and is not control of a task.
- **A sweep takes one's own.** "All" (the bulk tool without numbers, the sidebar's "Archive
  all") acts on the person's own tasks; another person's task is tidied by naming it.
- **Who tidied is recorded.** A tidy of a task by someone who does not own it is audited under
  their name with the owner's, and a stop says so in the task's terminal.
- **Consent is asked where the task is seen and heard where the owner is.** Request control is
  on the task's page and in the task's menu wherever it opens; the request reaches the owner
  as a notification and marks the task's row. A request waits without limit.
- **The giver can take the task back.** A hand-over on request records who gave control
  (`tasks.control_from`). That person takes the task back without asking, and it acts on
  GitHub as them again. The right ends when control moves again: handed on (the new giver is
  that owner), handed back, or passed from an absent owner.
- **An owner who is gone cannot consent, and only then does a request pass control**
  (0060's rule, kept): a disabled or deleted account, or one without write access to the
  project. An owner who is away keeps the task.
- **Asking, consenting and taking back are a person's own acts.** No agent tool does them.

## Alternatives

- **Keep stop, archive and delete with the owner** (0060). Reversed by the person: a team's
  lists and seats are shared, and ending a task takes nothing of its owner's.
- **Open them to readers too.** A reader changes nothing in a project (ACC-007).
- **Make pause tidy as well.** A pause holds a task for whoever paused it to resume, and resume
  is steering; a task that must not run is stopped.
- **Change `load_task`'s parameter to the new type at every call.** Forty call sites, and each
  task function a parallel change adds would have to pick a class; keeping "above view is
  control" lets them stay the owner's unsaid.
- **Consent as a grant to type beside the owner.** Two people steering an agent that acts on
  GitHub as one of them is what 0060 exists to prevent.
- **A history of givers, each able to take back.** Whoever handed a task on consented to its
  next owner; a right that reaches past them would take control from someone who never dealt
  with its holder.
- **A request that passes control after a time, or an administrator who takes it.** Both are
  control without consent.
- **Create the onboarding task for the sandbox's creator and refuse everyone else.** A
  creator who is away would leave no task to ask control of, and a task created by a refused
  start is a side effect a refusal must not have.
- **Give browser tabs owners.** A tab is opened by an agent or a person in one shared
  Chromium and nothing records whose it is; gating a tab while a task asks for an action in
  it would stop a teammate from helping and leave every other tab as it is.
- **Tools that ask for control or hand over.** A Superagent reads transcripts and web pages;
  consent given on a sentence read there is not the owner's.

## Consequences

- A person can lose a running task to a teammate's Stop or Delete. The confirmation names the
  owner, the audit names who did it, and a stopped task can be continued by its owner; a
  deleted one is gone, as for the owner's own delete.
- A person's agents can do the same for them: an agent of one person's task stops or deletes
  another person's task when its own work says so. Tasks could do that to every task of the
  project before 0060; the owner's audit trail now says who.
- A new owner holds a task that its giver can take back at any time; the hand-over's notice
  says so.
- A Superagent no longer passes its person's words to a teammate's task: the person asks the
  teammate, or asks for control.
- What 0060 said of the disk stays true: a shell in the sandbox repository reaches every
  worktree, so the rule guards Yulius's own ways in, not the files. And any writer may still
  change an automation, whose tasks are its creator's; that is reported for a decision of its
  own.
- Tests and documents that named stop, archive and delete among the owner's actions say the
  new rule (SPEC-070 R3 and R7, SPEC-066 MGT-008..010, `docs/api.md`, `docs/product.md`,
  `docs/architecture.md`).
