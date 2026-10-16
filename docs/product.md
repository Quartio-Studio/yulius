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

- **Projects and sandboxes**: import a GitHub repository, start a playground without GitHub, or
  onboard a repository from a local machine's Claude sessions with `yulius onboard`; each gets a
  contained sandbox with its own system layer, Docker engine (SPEC-187: `docker build`, `run` and
  `compose` work for agents, the Superagent, shells and CI jobs), home, shells per user, browser
  and editor. The CLI
  only uploads: the onboarding task that adapts the repository to the Yulius standards is created
  and started when someone clicks Start onboarding on the sandbox page. It also lays the
  project's gate in its own pull request: tests, lint and format, a dead-code and
  unused-dependency check, and a Yulius CI job for every project, with existing findings
  baselined. The first task therefore starts in the discipline. Onboarding is mandatory:
  until it has finished, the sandbox's new-task page shows only what it will do and its button,
  no task input. A playground needs none.
- **Playgrounds**: a team tries Yulius on its own code before it connects GitHub. On a fresh
  instance Home offers it first: drop a project folder (a repository keeps its branches and
  history) or start from a template, and tasks start a minute later. Its Git stays on the server
  and nothing is pushed anywhere; its tasks merge into its own `dev` with `delivery_merge` (no
  pull request), on green Yulius CI when the project has CI (SPEC-163). It is its maker's and the superadmin's. Connecting GitHub is
  optional and can wait (SPEC-099). A local checkout without GitHub, with its Claude Code
  sessions, comes in with `yulius onboard --playground`, and removing a playground can delete its
  code from the server for good, which frees its place among its maker's 10 (SPEC-171). Once GitHub
  is connected, an admin of a playground moves it into an empty repository of the instance owner
  ("Move to GitHub…" in its sandbox menu): its branches and tags are pushed there, it becomes a
  GitHub sandbox with the same tasks and history, and its access follows GitHub (SPEC-176).
- **Superagent** (formerly the Agent, and before that the Cockpit): every user's own plain Claude
  Code session per sandbox, in the right tool panel beside any page (first in the tools rail, under
  the tools toggle; the top bar has no button of its own since SPEC-132), for quick work
  such as setting up automations or asking a quick question; anything more complex, such as
  writing code, belongs in a task, and its panel says so. `/clear` starts over, it cannot be
  deleted and comes back with its conversation. It manages the project for its owner: asked to
  "create a task from that" or to do something "every day at 4am", it creates the task or the
  automation through Yulius's tools, and it stops, archives or deletes tasks, runs or deletes
  automations and enables the ready-made ones, asking the person before each change. Task agents
  can do the same for their task's owner, except to their own task. When a usage limit cuts its
  turn, it moves to a subscription with capacity and goes on with the turn, as a task does; the
  panel follows it. It has the project's MCP servers, as a task does, and an agent secret one of
  them or a command prints shows there as `[secret:NAME]` (SPEC-147). It uses the agent vault as
  a task does, never seeing a value: it is behind the egress proxy, so its requests to a secret's
  bound hosts carry it (`.mcp.json` servers with placeholders work too); it copies allowed secrets
  into the repository's git-ignored `.env` and asks for hosts, each asked under the person's name
  ("alice's Superagent") for a person to allow; and it tells whether a key the person holds in a
  file is the one in the vault, from the file's SHA-256 (SPEC-178, SPEC-186). After an update
  that gives it new tools, an open Superagent restarts by itself once it is idle and goes on with
  its conversation.
- **MCP servers**: configured once for the instance or a sandbox (MCP page) and attached to every
  task run on every subscription and to the Superagent, their secrets taken from the agent
  secrets. A repository's
  `.mcp.json` servers join its sandbox's list by themselves, with `${VAR}` becoming the agent
  secret VAR's placeholder (the egress proxy swaps the value in on its bound hosts), and the
  Superagent never asks about them.
- **Sandbox tabs**: the top row starts with All sandboxes, then a tab per sandbox in the order
  the person chose: with a mouse or pen each shows a drag grip, as the Notes tabs do, and each
  says it can be clicked or dragged; its menu moves it too.
- **Sidebar**: the sandbox pages used every day (Tasks, Git, CI/CD — one page whose CI (tests) and CD (deployment) tabs work like Secrets' Team and Agent tabs —, Secrets, Dashboard — with Overview and Pipeline (stages) tabs —; the Superagent is on the right),
  then "More" for Sessions, Workflows, Skills, Automations and MCP, then the active tasks.
- **Tasks**: each task runs a real Claude Code CLI in a PTY on its own git worktree. Sessions
  outlive server restarts. Task intake splits mixed requests, holds scope changes and chains
  dependent tasks, also by a logical rule such as "#12 done and #13 done", which the composer
  builds from picked conditions (a task, its state, all or any).
  A task that waits and has not started yet takes queued messages: each is added to its request,
  so the agent gets the correction when the task starts. The request and the message box show
  files as placeholders (`Image N`) that open on a click (a double-click in the box); images pasted or dropped into a message
  land where they are put and move like a character, as in the composer.
  A **supertask** (the composer's start menu, for broad work such as security hardening or
  one-shotting the product) waits for the running tasks of its project, then runs alone: the project's other tasks wait for it,
  also those queued or chained before it, and say so. When such broad work is started another way
  (Start now, after a task, by a rule), Jev stops it and the composer offers to run it as a
  supertask instead; a request that splits into separate tasks ("fix all GitHub issues") is not
  stopped (SPEC-188).
  The Tasks page's **Graph** (also from the sidebar's Working group) shows that structure at a
  glance: the queue in the order tasks get a free session, the chains, start rules and supertask
  holds as arrows between tasks, the prepared tasks and those on their own (SPEC-164).
  A task can also be **prepared**: written now and started later, when the person presses Start
  now ("When I decide") or by itself at a date and time they pick ("At a time…"). Until then it
  holds no session, can be rescheduled or cancelled, and sits with the queued tasks.
  Until a task starts, how it starts can change: the start chip in a waiting task's view offers
  the same choices as New task (after a task, a rule, When I decide, a time, Start now or
  Supertask); a task that started keeps its start.
  An image or file pasted or dropped into the prompt shows in a tray above the text, where an
  image opens large (fitted or at its actual size), and sits in the text as a one-line
  placeholder (`Image 1`) at the caret. The placeholder moves and deletes like a character, or is
  placed from the tray, and the agent gets the file at that place (a file left without one comes
  after the text).
  A half-written task (its prompt, attached files and choices) waits in the composer while
  people look elsewhere, also across a reload, until it starts.
  A task is its owner's: Yulius acts on GitHub for it with their credentials (the agent's own Git
  and `gh` use a narrowed App token), so only they change it. Its owner can **pause** a running task and resume it later: the agent
  stops where it is without being told (as an update pauses every task) and goes on from there; a
  paused task, and one an update holds, reads Paused in the lists and says why. Others
  watch it as its live transcript (printed as the Sessions view prints a session, in the
  terminal's colours, following its newest lines until they scroll up), see whose it is
  in the view and in the sidebar, and can ask the owner for control; once handed over, it acts as
  the new owner.
- **Subscription pool**: Claude accounts are used as one self-managing pool; tasks are
  subscription-agnostic, switches and pauses are invisible to agents. Its Capacity card says what
  is left, how fast it burns, the tokens it used and what they would cost at Claude API list
  prices, with a 7-day chart for each.
- **Delivery discipline**: spec when it matters, effort-weighted steps, independent review
  (findings in the change's own code are fixed before merge, never deferred to an issue),
  merge on green into `dev`, cleanup; promotion to `main` is a person's. Profiles live in
  `yulius.toml`, with the branch policy (typed branches, conventional titles, `dev` → `main`
  promotions) that Yulius checks before every merge.
- **Yulius CI**: the projects' only CI; it runs on the VM, no GitHub Actions. A job uses an agent
  secret only when the integration branch's `[[ci.jobs]]` entry names it (`secrets`), as a
  placeholder through the egress proxy toward the secret's bound hosts, audited as `ci:<run>`;
  every pull request runs Yulius's own `security` job, granted `VIDOC_API_KEY`, which scans with
  Vidoc once the key works (#470). A job that checked nothing exits with `$YULIUS_CI_SKIP_CODE` and
  reads **Skipped**, never Passed; it blocks no merge (ADR-0133).
- **Pipeline (stages)**: every stage of tasks, CI runs and deployments is measured as it happens
  and kept 180 days (SPEC-137): a task's waits, states, steps (with their tokens), runs, pauses,
  pull request and merge waits; a CI run's queue (and why), jobs and the stages their output names
  (`::yulius-stage`, with test counts); a deploy's confirmation, promotion, deploy and verify; what a
  CI job or deploy used (CPU time, peak memory, bytes written; SPEC-154). The Dashboard's Pipeline
  (stages) tab (`?section=pipeline`; `/pipeline` still opens it) shows deployment frequency, lead
  time, change failure rate and time to restore for dev and production, any stage's daily p50 and
  p90 and the stage table (with CPU and memory p50 · p90) with CSV/JSON export; a CI run, a deploy
  and a task show their own waterfall (a time axis, a legend, children folded under their span; a
  CI run's jobs one row each with their wait for a job slot), and the Dashboard's Overview tab the
  overview.
- **CD (deployment)**: the CI/CD page's second tab, with CI's discipline. Every project defines dev and
  production in `yulius.toml` (`[cd.dev]`, `[cd.prod]`), each with deploy and verify scripts
  committed under `scripts/cd/`, or explicitly empty while it deploys nothing yet. Yulius runs the
  exact commits GitHub reports for the protected branches, dev inside the sandbox and production
  outside it, with the agent secrets a person allowed for each, and shows each run on the CD
  (deployment) page; a confirmed deploy opens its dialog at once, saying what it checks and
  promotes, and that dialog becomes the run. A merge deploys nothing: a person or an agent
  (`cd_deploy`) deploys dev when it is wanted, or the ready-made "Deploy dev every night"
  automation deploys and tests it. Production (with its promotion) is always a person's: an
  administrator types the project's slug, and on GitHub a ruleset only repository admins bypass
  keeps the production branch out of reach of the agents' own token, which is never a person's
  (onboarding says when that ruleset is missing, and its agent adds it with `production_protect`
  as the person who started onboarding when they are a repository admin). Agents build and improve CD through pull requests but
  cannot change what a deploy runs, and they ask a person with `needs_you` for what only a person
  can give (a machine's credentials). Onboarding finds how a project deploys today and writes it
  down this way; "Set up CD with an agent" does the same later, through a task. For Yulius itself
  dev is empty until it has a dev machine, and production is the push to `main` that instances
  install by themselves.
- **Browser panel**: the sandbox's real Chrome streamed as video so people can pass captchas
  and sign-ins for the agent.
- **Automations**: tasks on a schedule. Three ready-made ones, off until a person enables them in
  one click on the Automations page: "Keep the docs up to date", "Work through open issues" (the
  core team's issues ranked, and the 5 most important actionable ones that no running task owns
  verified in parallel and given a task each, no new issues) and
  "Deploy dev every night" (a CD run of dev, tested, a failure filed as an issue),
  nightly at 03:00 CEST. An automation's tasks start like New task's: now, as a supertask, after
  another task or when a rule is true, or wait prepared until the person presses Start now ("When
  I decide"), picked with New task's start chip beside Run now and Save;
  Run now starts its task the way the chip shows.
- **Notes**: in the tools rail, a notepad per sandbox and one for the whole team, shared and saved
  while people type; a teammate's edit shows up live, and two edits at once end in a choice. A
  third, **Me**, is each person's own: nobody else sees it, and it is the same in every sandbox.
  The tabs go in any order (Team first until moved); each shows a drag grip and says it can be
  clicked or dragged. Selections are drawn rounded and glide, and
  teammates who have a shared notepad open show as a caret with their name and their selection
  in their own colour.
- **Editor**: Monaco over the sandbox's files for people with write access. Like Notes, its
  selection is drawn rounded and glides, people who have the same file open see each other's
  caret, name and selection in their Notes colour (awareness only: unsaved text stays each
  person's own), and its tabs go in any order. The file in front follows the disk: what
  teammates, agents and shells write shows within about 2 s while the person has not edited it
  (SPEC-181).
- **Editor, terminals, Git view, automations, workflow catalog** in one web UI, whose menus fit
  the window and filter as people type when they are long; the start menu shows five tasks at a
  time and says how many more it scrolls to. Messages are
  withdrawn for now (SPEC-028): agents learn about checks, delivery and armed merges from
  `yulius wait`, `ci_status` and `delivery_status`. An agent removes the files its task created
  with `yulius remove` instead of asking a person when Claude Code stops an `rm` (SPEC-192).
- **Agent primitives as code**: Yulius's own skills are APM packages with a lockfile, deployed to
  every harness and checked for drift; onboarding codifies a project's conventions as primitives.
- **Secrets vault**: secrets are a title, a value and notes. Team secrets are for people; agent
  secrets are used by agents, who never see them. Reading a value needs your own two-factor code
  (set up once, mandatory); an administrator resets it for someone who lost their authenticator.
  The people who manage an agent secret can read it too. A shown value stays masked until its eye
  button is pressed; it copies without being shown. An agent secret is added with a name, a value
  and a description: agents find the service's host and ask for it, and a person allows it with
  one click (SPEC-121). A project's keys live in a git-ignored `.env` beside a tracked
  `.env.example`; when its code needs a secret's real value, the agent asks Yulius to copy it
  there, and a person allows that once per secret (SPEC-129). A multi-line key (PEM, OpenSSH) is
  pasted into the value as it is: the field keeps a key's line breaks as literal `\n`, and it lands
  in the `.env` as its file, byte for byte (SPEC-186, SPEC-193). A key saved before that, its line
  breaks lost, is marked "Paste again" for the people who manage it.
- **Updates**: Yulius updates itself from `main` without losing running work. The superadmin
  starts an update from the Update button beside their account button in the sidebar (SPEC-133).
  Once the new version is built, every task is frozen at once, mid-turn, and thawed after the
  restart; the agents don't notice (SPEC-160).

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
