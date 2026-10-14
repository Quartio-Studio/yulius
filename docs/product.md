# Product summary

Yulius is a self-hosted coding workspace for the Quartio team, written in Rust and inspired by
Cezar and Open Mercato Cloud. It runs native Claude Code sessions, one per task, inside
contained sandboxes on a single Linux VM, and enforces engineering discipline around them.
Yulius develops itself with Yulius.

The authoritative product brief is [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md); this
page is the short version for anyone who must know the whole product before building.

## Who it is for

A small team (2-5 people; the founders are superadmins) that works with several Claude
subscriptions and many repositories and wants agents to deliver autonomously, with people
watching, steering and unblocking.

## What it does

- **Projects and sandboxes**: import a GitHub repository (or onboard it from a local machine's
  Claude sessions with `yulius onboard`); each gets a contained sandbox with its own system
  layer, home, shells per user, browser and editor. The CLI only uploads: the onboarding task that
  adapts the repository to the Yulius standards starts when someone clicks Start onboarding on the
  sandbox page. Onboarding is mandatory: until it has finished, the sandbox's new-task page shows
  only what it will do and its button, no task input.
- **Superagent** (formerly the Agent, and before that the Cockpit): every user's own plain Claude
  Code session per sandbox, in the right tool panel beside any page (top-bar Superagent button,
  first in the tools rail), for quick work
  such as setting up automations or asking a quick question; anything more complex, such as
  writing code, belongs in a task, and its panel says so. `/clear` starts over, it cannot be
  deleted and comes back with its conversation. It manages the project for its owner: asked to
  "create a task from that" or to do something "every day at 4am", it creates the task or the
  automation through Yulius's tools, and it stops, archives or deletes tasks, runs or deletes
  automations and enables the ready-made ones, asking the person before each change. Task agents
  can do the same for their task's owner, except to their own task. When a usage limit cuts its
  turn, it moves to a subscription with capacity and goes on with the turn, as a task does; the
  panel follows it.
- **MCP servers**: configured once for the instance or a sandbox (MCP page) and attached to every
  task run on every subscription, their secrets taken from the agent secrets. A repository's
  `.mcp.json` servers join its sandbox's list by themselves, with `${VAR}` becoming the agent
  secret VAR's placeholder (the egress proxy swaps the value in on its bound hosts), and the
  Superagent never asks about them.
- **Sidebar**: the sandbox pages used every day (Tasks, Git, CI (tests), Secrets, Dashboard; the Superagent is on the right),
  then "More" for Sessions, Workflows, Skills, Automations and MCP, then the active tasks.
- **Tasks**: each task runs a real Claude Code CLI in a PTY on its own git worktree. Sessions
  outlive server restarts. Task intake splits mixed requests, holds scope changes and chains
  dependent tasks, also by a logical rule such as "#12 done and #13 done", which the composer
  builds from picked conditions (a task, its state, all or any).
  A task that waits and has not started yet takes queued messages: each is added to its request,
  so the agent gets the correction when the task starts.
  A **supertask** (the composer's start menu, for broad work such as security hardening) waits
  for the running tasks of its project, then runs alone: the project's other tasks wait for it,
  also those queued or chained before it, and say so.
  A half-written task (its prompt, attached files and choices) waits in the composer while
  people look elsewhere, also across a reload, until it starts.
  A task is its owner's: it acts on GitHub with their credentials, so only they change it. Others
  see it read-only and can ask the owner for control; once handed over, it acts as the new owner.
- **Subscription pool**: Claude accounts are used as one self-managing pool; tasks are
  subscription-agnostic, switches and pauses are invisible to agents.
- **Delivery discipline**: spec when it matters, effort-weighted steps, independent review,
  merge on green into `dev`, promotion to `main`, deploy and verify, cleanup. Profiles live in
  `yulius.toml`, with the branch policy (typed branches, conventional titles, `dev` → `main`
  promotions) that Yulius checks before every merge.
- **Yulius CI**: the projects' only CI; it runs on the VM, no GitHub Actions.
- **Browser panel**: the sandbox's real Chrome streamed as video so people can pass captchas
  and sign-ins for the agent.
- **Automations**: tasks on a schedule. Two ready-made ones, off until a person enables them in one
  click on the Automations page: "Keep the docs up to date" and "Work through open issues" (the
  core team's issues verified in parallel, a Fix task for each actionable one, no new issues),
  nightly at 03:00 CEST. An automation's tasks start like New task's: now, as a supertask, after
  another task or when a rule is true, picked with New task's start chip beside Run now and Save;
  Run now starts its task the way the chip shows.
- **Notes**: in the tools rail, a notepad per sandbox and one for the whole team, shared and saved
  while people type; a teammate's edit shows up live, and two edits at once end in a choice. A
  third, **Me**, is each person's own: nobody else sees it, and it is the same in every sandbox.
- **Editor, terminals, Git view, automations, workflow catalog** in one web UI, whose menus fit
  the window and filter as people type when they are long; the start menu shows five tasks at a
  time and says how many more it scrolls to. Messages are
  withdrawn for now (SPEC-028): agents learn about checks, delivery and armed merges from
  `yulius wait`, `ci_status` and `delivery_status`.
- **Agent primitives as code**: Yulius's own skills are APM packages with a lockfile, deployed to
  every harness and checked for drift; onboarding codifies a project's conventions as primitives.
- **Secrets vault**: secrets are a title, a value and notes. Team secrets are for people; agent
  secrets are used by agents, who never see them. Reading a value needs your own two-factor code
  (set up once, mandatory); an administrator resets it for someone who lost their authenticator.
  The people who manage an agent secret can read it too.
- **Updates**: Yulius updates itself from `main` without losing running work.

## Principles

Agents are real CLIs; sessions outlive the server; GitHub is the authority for permissions;
coordination goes through one MCP and hook protocol; every queue, stream, retry and wait is
bounded and owned; features have specs, research and questions do not.

## Where to read more

- Architecture: [architecture.md](architecture.md); decisions: [decisions](decisions/README.md).
- Requirements and evidence: [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md),
  [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md),
  [SPEC-003](../.ai/specs/SPEC-003-traceability.md).
- Operations: [operations.md](operations.md); API: [api.md](api.md).
- Change history: [changes.md](changes.md).
