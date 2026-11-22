# Product summary

Yulius is a self-hosted coding workspace for the Quartio team, written in Rust and inspired by
Cezar and Open Mercato Cloud. It runs native Claude Code sessions, one per task, inside
contained sandboxes on a single Linux VM, and enforces engineering discipline around them.
Yulius develops itself with Yulius. Its pull requests get independent results for check, lint, macOS, docs and dependencies. Pull requests and merge trains also run the Rust contracts that read changed repository files. The merged revision runs both Rust test partitions from one build and runs the browser suite. [Decision 0438](decisions/0438-quality-passes-have-own-jobs-and-test-partitions-share-a-build.md)

Finished tasks keep their last observed pull request state while GitHub is unavailable. The instance shows when merges, CI status updates and staging can use GitHub again. [SPEC-608](../.ai/specs/SPEC-608-a-github-rate-limit-must-not-show-finish.md)

A GitHub App subscribed to PR openings triggers CI for hand-opened pull requests as soon as GitHub delivers the event and a place is available within the sandbox’s 30 open PRs. [Decision 0441](decisions/0441-github-pr-openings-trigger-exact-head-ci.md)

It also runs native Codex CLI sessions on Codex (OpenAI) subscriptions, chosen per task or
automation. GPT-6.2 Astra is the default model. Every Codex model inherits High effort.
A task's explicit effort takes precedence, including Extra high and Max on GPT-6.2 Astra and Sol.
[SPEC-607](../.ai/specs/SPEC-607-codex-tasks-run-at-high-effort-unless-th.md)

Continue resumes a stopped Codex task in its saved conversation. Reopening a closed Codex conversation shows Waiting until its next turn. [SPEC-590](../.ai/specs/SPEC-590-look-bro-the-taks-says-it-working-but-th.md)

Automatic Codex placement uses the eligible subscription with the most quota left in its tightest
window. An idle Codex task moved to another subscription keeps Waiting or Done.
[SPEC-611](../.ai/specs/SPEC-611-a-moved-task-is-not-shown-as-working-and.md)

Codex tasks keep their selected model when Yulius sends a message near a subscription limit.
New and resumed sessions hide the rate-limit model switch reminder. A visible reminder holds
automated messages until the picker is dismissed. [SPEC-603](../.ai/specs/SPEC-603-codex-s-rate-limit-prompt-must-not-switc.md)

Tasks created by the CLI or either Superagent default to Codex when harness and model are omitted.
A task created with a `gpt-*` model and no explicit agent selects Codex.
A task keeps its agent for life, and a Codex task moves only between Codex subscriptions. On ChatGPT Pro 500,
Codex defaults to Ultrafast for models that support it. A Codex
subscription signs in by device code, by an imported `auth.json` of a `codex login` made elsewhere,
or with an OpenAI API key, and Yulius provides the pinned Codex CLI
(0.170.0) to every Codex run. The founders accepted this on 2026-11-16 (`F 2026-11-16 Codex`,
outside pull request #1146 by studenciak00owl), reversing the Codex part of their Claude-only
decision `F 2026-10-08`; provider switching and session translation stay out
([SPEC-503](../.ai/specs/SPEC-503-accept-outside-pr-1146-codex-returns-as.md),
[ADR-0355](decisions/0355-codex-returns-as-a-second-harness-with-a-pinned-cli.md)).

It runs native Antigravity CLI sessions on Antigravity (Google) subscriptions too, on Gemini 4
Neon at high effort unless a task names another Gemini model such as Gemini 4 Carbon
(`F 2026-11-17 Antigravity`, task #487; `F 2026-11-19 Neon`, task #501). An Antigravity
subscription signs in with the CLI's own Google sign-in or a Gemini API key, an Antigravity task
moves only between Antigravity subscriptions, and Yulius provides the pinned Antigravity CLI
(1.5.12)
([SPEC-505](../.ai/specs/SPEC-505-implement-antigravity-cli-next-to-claude.md),
[SPEC-519](../.ai/specs/SPEC-519-this-is-a-test-task-confirm-everything-w.md),
[ADR-0361](decisions/0361-antigravity-is-a-third-harness-picked-at-random-by-default.md),
[ADR-0373](decisions/0373-gemini-4-neon-is-antigravitys-default-model-on-agy-1-5-12.md)). An
Antigravity task stays Working while its command runs in the background or its agent resumes by
itself ([SPEC-528](../.ai/specs/SPEC-528-antigravity-tasks-show-needs-its-person.md)). Reopening its
conversation without a prompt leaves it idle and eligible for normal parking or closure, even
with a Gemini announcement visible. A
task created with the composer’s `Random` setting or an explicit `auto` harness and
without a model that implies one draws uniformly across all added harnesses (`claude`, `codex`,
`antigravity`) that currently have at least one non-exhausted subscription — one equal chance per
eligible harness regardless of subscription count, excluding harnesses with zero or only finished
subscriptions while another has capacity — and keeps that assigned harness for life (`harness_choice:
random`, exported in analytics); a running task agent's unadorned follow-up keeps the calling
task's harness, and an automation defaults to `Random` (`auto`, drawn for each run) or runs the
agent it names (`F 2026-11-19 Random harness`,
[SPEC-520](../.ai/specs/SPEC-520-i-dont-know-why-but-i-dont-see-random-ha.md),
[ADR-0377](decisions/0377-random-draws-uniformly-across-harnesses-with-available-subscriptions.md),
superseding the no-draw part of [SPEC-514](../.ai/specs/SPEC-514-make-sure-harnesses-doesnt-change-themse.md)
/ [ADR-0368](decisions/0368-a-tasks-agent-is-set-never-drawn.md)).

The composer and Settings start on Codex default in a browser without a saved model. A saved Random choice or specific model takes precedence. [SPEC-587](../.ai/specs/SPEC-587-codex-should-be-default-for-example-bart.md)

The composer and Settings offer individual Codex and Gemini models grouped by provider. Each model
shows its supported effort levels. Choosing a model that cannot use the saved effort restores the
High default. [SPEC-521](../.ai/specs/SPEC-521-i-should-be-able-to-selected-every-model.md)

Goal tasks continue their remaining plan steps, including post-merge verification, through the existing turn-end retry budget. Owners see a request when their action is needed or the bounded attempts fail. [SPEC-589](../.ai/specs/SPEC-589-it-shouldnt-ask-for-things-like-that-fix.md)

The authoritative product brief is [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md); this
page is the short version for anyone who must know the whole product before building.

Yulius's own Rust builds share compiled artifacts across task worktrees and CI lanes on the project sandbox's disk. [ADR-0434](decisions/0434-keep-a-local-compiler-cache-in-each-project-sandbox.md)

Agent and CI build scratch is disk-backed. Deliveries check their tests by hand. A task can request a mutation campaign by name. [Decision 0439](decisions/0439-delivery-checks-mutations-by-hand-and-builds-on-disk.md)

The Status panel shows how many GitHub App requests remain and which operations used them. At the reserve, task display yields requests to delivery. A completed pull request can still be read locally. [ADR-0447](decisions/0447-task-display-shares-github-reads-and-yields-to-delivery.md)

## Who it is for

A small team (2-5 people; the founders are superadmins) that works with several Claude
subscriptions and many repositories and wants agents to deliver autonomously, with people
watching, steering and unblocking.

## What it does

- **First run** (SPEC-283, ADR-0211): a numbered setup wizard (superadmin with the setup token,
  recovery code, the optional Jev and Vidoc keys, then the first Claude subscription, skippable)
  says on each card what the step does and what comes next; Home's empty view lists the
  getting-started steps (Connect Claude, first sandbox, GitHub, optional) with their state.
  Creating the GitHub App opens GitHub in a new tab while the Yulius tab waits and then lists the
  next steps (install on repositories, import or create a sandbox, admit users).
- **Projects and sandboxes**: import a GitHub repository, start a playground without GitHub, or
  onboard a repository from a local machine's Claude sessions with `yulius onboard`; each gets a
  contained sandbox with its own system layer, Docker engine (SPEC-187: `docker build`, `run` and
  `compose` work for agents, the Superagent, shells and CI jobs; SPEC-196: in sessions started
  before too, `yulius doctor --docker-smoke` proves it on a host; SPEC-203: `sudo yulius host
  setup` installs what it needs on a fresh host, every update re-checks it, and Settings →
  Instance shows whether sandboxes get Docker and runs the smoke check; SPEC-279: production
  deploys get a Docker engine of their own that no agent reaches), home, shells per user,
  browser and editor. Files nothing needs any more (a removed sandbox's worktrees and system
  layer, a done task's build cache) are deleted by themselves after every start and hourly, and
  the superadmin reviews and deletes the rest in Settings → Instance → Leftovers or with
  `yulius leftovers` (SPEC-215). Cargo build directories of worktrees and CI checkouts lose what
  nobody used for two days and stay under a cap, never while a build holds them; when the
  server's disk runs low Yulius sweeps harder, prunes idle caches and every page shows a warning
  (SPEC-276). When the egress proxy refuses a sandbox at its share, or its relay toward Yulius or
  the proxy itself is full, the sandbox's people see a warning naming the task whose run holds the
  most connections, notified once until it has room again (SPEC-361). On Linux, a frozen agent's established
  opaque HTTP/1.1 CONNECT tunnel keeps its original peers and pending bytes during an orderly
  server update, including through a surviving sandbox holder. Multiplexed HTTP/2 connections
  to the proxy reconnect. Intercepted requests reconnect through the successor's host-bound
  credential policy (SPEC-577). Removing a sandbox asks
  whether to delete its files permanently or keep them; kept
  ones are listed with their size for a later deletion. `yulius onboard`
  only uploads: the onboarding task that adapts the repository to the Yulius standards is created
  and started when someone clicks Start onboarding on the sandbox page, or when the person's own
  agent calls `onboarding_start` with an operate token (SPEC-247). It also lays the
  project's gate in its own pull request: tests, lint and format, a dead-code and
  unused-dependency check, and a Yulius CI job for every project, with existing findings
  baselined. The first task therefore starts in the discipline. Onboarding is mandatory:
  until it has finished, the sandbox's new-task page shows only what it will do and its button,
  no task input. A playground needs none.
- **Remote control from a laptop** (SPEC-247, [guide](remote-control.md)): a person makes an API
  token on Settings → Connect your local agent (operate or read only, shown once, revocable, last
  use shown) and adds Yulius to their own Claude Code with one `claude mcp add` command, or signs
  the `yulius` CLI in with `yulius remote login`. From there, without the web UI, they import a
  repository, start its onboarding, read its files, set up its CI with a change pull request
  merged once Yulius CI passed, create, message, restart, pause, stop, archive and delete tasks,
  follow their transcripts, pull requests and CI, run dev deploys and manage automations and
  agent secrets' bindings, always within their own access to each project. Production deploys
  and secret values stay with the web UI (and `yulius secrets import`).
- **Onboard a laptop project with one request** (SPEC-256): "onboard this into Yulius" to the
  local agent is enough. With the operate token it uploads the checkout and Claude Code sessions,
  finds the keys the project builds and deploys with and imports them by file, unattended and
  already usable by agents in `.env` files (values never pass through the agent), and starts
  onboarding with its hand-over (which key is which, where deploys run). The onboarding task uses
  those keys before asking anyone.
- **Operator state moves with the project** (SPEC-271): a deploy tool's live state on the laptop
  (an inventory database, rollout folders, configs) is sent as an encrypted agent state
  (`yulius secrets import --state NAME=DIR`, up to 64 MiB packed), restored by a task into the
  sandbox home and saved back after each rollout. Once restored, the sandbox's copy is the only
  writer: a second send is refused unless the person replaces it on purpose, and
  `yulius secrets state-get` brings the newest copy back. CD deploys use it too (SPEC-278): a
  `[cd.<name>]` names its states, each run restores the vault's newest version and saves it back
  after the deploy, one writer at a time.
- **Project variables** (SPEC-275): a project's non-secret values (regions, environment names,
  ids, base URLs, contact addresses) are shown in plain on the Secrets page's Variables tab and
  are in every task's environment and in CD runs' (dev: all; production: those its `[cd.<name>]`
  names, SPEC-278). The laptop agent sets them with `yulius vars`, and the secrets
  import sends a `.env`'s plain values there instead of making them secrets.
- **Resource limits fit the host** (SPEC-459): how many sessions, CI jobs and CI runs per
  sandbox run at once, a sandbox's memory and disk quota, and the disk a deploy may fill are
  sized from the host's CPUs, memory and data disk, with no number in the service unit. Settings
  → Instance → Resource limits shows the host, each value, where it comes from (host-sized, the
  unit, a setting) and the formula; the superadmin saves a number there (it wins over the unit),
  in force at once except the sandbox memory, which waits for a restart. A number pinned on a
  host of another size is noticed: the page and the start log name it, the host it was
  configured on and the host-sized value, and one click takes the host-sized values or keeps the
  numbers. After `yulius instance import` the first start does that by itself.
- **Move or copy the whole instance** (SPEC-329): `sudo yulius instance export | ssh root@new-vm
  yulius instance import` pauses every Yulius process, announces the exact size, and streams the
  whole instance (database, keys, sandboxes with uncommitted work and Claude sessions, installed
  packages, Docker data) without writing anything on the old VM's disk; the new VM installs it as
  it arrives, checks it, and `yulius host setup` starts it. `--move` keeps the old instance frozen
  and disabled. A copy can also go to a file and be installed later.
- **Move the instance to another host with one command** (SPEC-460): `sudo yulius instance migrate
  root@new-host` on the old host checks and sets up the new one (a data disk, the old host's
  packages and apt sources, software outside apt, the unit with its settings and key files, the
  update helper, Caddy, ufw), copies the data while the old instance still runs (SPEC-465: in
  passes into a staged copy, resumable after a broken link), freezes it only to send what changed
  since, proves the copy (entries, bytes and a fingerprint per folder, equal on both hosts),
  starts the new instance, redirects the old URL and removes its own key. The estimated freeze is
  shown before it begins and the optional longest freeze in Settings or `--max-downtime <minutes>` bounds it. People are asked only in the
  plan, before anything changes (the SSH sign-in, a DNS record, the GitHub App's callback). Any
  failure before the new instance runs thaws the old one; `--abort` goes back. Tasks that were
  live come back by themselves on the new host. A superadmin can also move from **Settings →
  Instance → Move to another host** (SPEC-470): the plan with everything a person has to do (and
  the key line to add on the new host), "Run migration" behind the host typed by hand, the
  checklist as it grows, and an abort that says what it does; root runs the same command through
  a request file, as it installs updates, once root has enabled that helper. While the instance
  is frozen the page says so and keeps trying, and the new instance's page shows the finished
  checklist.
- **The instance knows whether GitHub accepts its address** (SPEC-466): after a move to another
  URL GitHub sign-in needs the new callback URL in the GitHub App, which only a person can add.
  Yulius asks GitHub whether it is there, and Settings → Instance (card Address) gives the one link
  and the two values to paste until it is; the move's plan checks the new URL before the cutover,
  and the sign-in page says why GitHub sign-in cannot complete meanwhile (the password works). The
  card also says where the instance came from and what its first start did with the tasks.
  `yulius remote` on a laptop follows the old URL's redirect to the new one by itself.
- **Playgrounds**: a team tries Yulius on its own code before it connects GitHub. On a fresh
  instance Home offers it first: drop a project folder (a repository keeps its branches and
  history) or start from a template, and tasks start a minute later. Its Git stays on the server
  and nothing is pushed anywhere; its tasks merge into its own `dev` with `delivery_merge` (no
  pull request), on green Yulius CI when the project has CI (SPEC-163). It is its maker's and the superadmin's, and its admins let teammates without GitHub try it: "Invite guests…" in its sandbox menu makes a link that lasts 1, 7 or 30 days, a teammate opens it and joins with a name, works on that playground's tasks only, sharing its workspace (so its runs get no agent secret from then on, while a guest's task can copy the Claude account's sign-in token, as the Invite dialog says), and is signed out and stopped when the link is revoked or ends (SPEC-410, the one exception to signing in with GitHub). Connecting GitHub is
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
  writing code, belongs in a task, and its panel says so. Like task runs, it answers in Claude
  Code's Concise output style unless someone picks another with `/output-style` (SPEC-210).
  Yulius switches the language servers of Claude Code plugins off for each run of either, and
  neither is asked to install one (SPEC-485).
  `/clear` starts over, it cannot be
  deleted and comes back with its conversation. When it cannot reach Yulius from its sandbox, its
  panel says so and Restart Superagent starts it again in place, keeping its conversation
  (SPEC-355). On a contained server it runs in Claude Code's
  bypass-permissions mode, as tasks do, not auto mode (SPEC-208). It manages the project for its owner: asked to
  "create a task from that" or to do something "every day at 4am", it creates the task or the
  automation through Yulius's tools, and it changes how a waiting task starts, stops, archives or
  deletes tasks (another person's too, when it is the one named), runs or deletes automations and enables the ready-made ones (asking the person
  before each change only on an uncontained server). A deleted task's attachments stay while
  another task's request or an automation's prompt names them and otherwise wait 7 days in a trash it restores them from
  (SPEC-396). Task agents
  can do the same for their task's owner, except to their own task. Told "tell #5 it may use the
  DO budget", the Superagent passes the message on to a task of its person's (task agents cannot,
  and another person's task takes messages from its owner only, SPEC-490): a task that has not
  started gets it in its request, a running one gets it typed into its session as "Message from
  alice (through their Superagent)" (or "(through their own agent)" when the person's own agent
  sends it through the remote MCP, SPEC-322), unless someone typed there in the last minute, text
  they typed and did not send may still be in its prompt box (however old, SPEC-480: such a
  message is not typed behind a person's draft, and for a message of Yulius's own the task
  says so after ten minutes), or the task still asks a person whether to trust its folder (SPEC-230).
  After that minute a confirmed empty Claude Code conversation input releases a waiting message.
  A draft visible in Claude Code’s input also holds the message, including one restored from
  a stash. The sender is told at once. An automatic message flags the session for its person
  ([SPEC-537](../.ai/specs/SPEC-537-preserve-restored-drafts-when-typing-int.md)). When a usage limit cuts its
  turn, it moves to a subscription with capacity and goes on with the turn, as a task does; the
  panel follows it. It has the project's MCP servers, as a task does, and an agent secret one of
  them or a command prints shows there as `[secret:NAME]` (SPEC-147). It uses the agent vault as
  a task does, never seeing a value: it is behind the egress proxy, so its requests to a secret's
  bound hosts carry it (`.mcp.json` servers with placeholders work too); it copies allowed secrets
  into the repository's git-ignored `.env` and asks for hosts, each asked under the person's name
  ("alice's Superagent") for a person to allow; and it tells whether a key the person holds in a
  file is the one in the vault, from the file's SHA-256 (SPEC-178, SPEC-186). It drives the
  sandbox's browser as a task does: it opens and reads pages, clicks, types and takes screenshots,
  signs in with an agent secret it never sees (only on the secret's hosts), and hands a tab to the
  person with a link; the Browser page shows its cursor as "alice's Superagent" (SPEC-437). Asked for work in
  another of the person's sandboxes ("make a task in the website repo for that"), it asks the
  person: the request shows above its session with the target, the whole prompt and its start
  options, and nothing reaches that sandbox until the person presses Create task there; Decline
  creates nothing, and no API token can approve (SPEC-335). The request stays there until the
  person answers it, also when they are away for hours or Yulius restarts; the Superagent reads
  how it ended, changes its text in place or withdraws it instead of asking again, lists another
  sandbox's tasks without asking, and leaves a decision only the person makes on the same panel
  as a question answered Yes or No (SPEC-487, [decision
  0333](decisions/0333-the-superagents-requests-to-its-person-last-until-answered.md)). Asked to
  deploy production, roll it back or promote, it asks on the same panel and the person, an
  administrator, approves by typing the project's slug as on the CD page; the deploy then runs as
  them. It starts a project's onboarding the same way, with a click
  ([decision 0334](decisions/0334-the-superagent-asks-for-production-on-its-panel-with-the-typed-slug.md)).
  Asked to set up a key it does not have, it asks for it on the same panel: the person types the
  value there, masked (a multi-line key keeps its line breaks), and it goes to the vault as
  theirs; the Superagent sees its name only, never the value
  ([decision 0336](decisions/0336-a-secrets-value-is-typed-by-its-person-on-the-superagents-panel.md)).
  It reads a task's screen, also the last one a crashed run left with its exit code, the tokens
  each task used as the Dashboard counts them, and how far its repository checkout is behind the
  integration branch (SPEC-487).
  A missing ability it reports in one call (`ability_report`): kept on the instance, with a link
  that opens it as a new issue in the Yulius repository for its person to read and submit
  ([decision 0337](decisions/0337-a-missing-ability-is-reported-to-the-yulius-repository-in-one-call.md)).
  What it and a task's agent may do, and why (SPEC-487; "yes" is with the person's rights, checked
  at the call):

  | Ability | Superagent | Task's agent | Why |
  | --- | --- | --- | --- |
  | Create, chain, schedule, stop, archive, delete a task; automations | yes (had) | yes (had) | SPEC-037: a follow-up runs as its own task |
  | Read a start rule back as stored; "after all current tasks" | yes (slice 1) | yes (slice 1) | Both create tasks with `task_create` |
  | Restart, pause, resume, start now | yes (slice 2) | no | A person speaks to their tasks; tasks do not steer each other (ADR-0178) |
  | Start a project's onboarding (also by continuing its onboarding task before it started) | behind the person's click in the panel (slice 8b) | no | SPEC-051 OBD-002, ADR-0038: a person decides that a repository is adapted |
  | Project variables: read | yes (slice 2) | in its environment (had) | They are not secrets |
  | Project variables: set, delete | yes (`variables_set`, `variable_delete`, slice 11; Configure) | no | Plain values, never a secret's; as a person's own agent through the remote MCP |
  | Rename, edit a prompt not started, reorder, close out, bulk actions | yes (slice 3) | no | As above |
  | Send a message past text the person left unsent in a task's prompt box | yes, for the task's owner (slice 3) | no | SPEC-480: the text is the person's; they decide that it goes out with the message |
  | Discard text a person left unsent in a task's prompt box | no | no | Yulius sees keys, not the box, and the keys that clear it (Esc, Ctrl+C) also interrupt a turn or end the session; the owner's message goes past it instead |
  | Message a task | yes (had) | no | ADR-0178 |
  | List open questions, see why a task needs attention | yes (slice 4) | yes, read only, without a held prompt's words (slice 4) | A task that sees another's question does not ask it again; a prompt Yulius held reached no agent |
  | Answer or dismiss a task's question for the person; decide a held prompt (send, dismiss, a new task now or after) | yes (slice 4) | no | The question is to a person; their Superagent answers when they say so |
  | Decide a held prompt by skipping steps or reverting the task | no: the person, on the task's page | no | Reverting discards the task's work and asks for the person's own confirmation there (TSK-013) |
  | Answer a question Claude Code asks in a task's terminal (a permission menu, a choice) | no: it reads the menu (`task_screen`) and tells its person | no | Typing into a task's terminal is its owner's control (SPEC-490); keys typed from a screen read a moment before could take another choice |
  | The end of a session (`session_read` tail) | yes (slice 4) | yes (slice 4) | Sessions are docs for both (SPEC-001 SES) |
  | CI: read runs and logs, run again | yes (slice 2) | yes (had) | A task needs its own checks |
  | CI: cancel, order | yes (slices 2, 5) | no | They change other tasks' runs |
  | CI: hold the queue | no: no page holds it either; cancel a run and run it again later | no | A held queue would hold every task's checks and merges; asked for in no case where cancel and run again did not do |
  | A task's delivery, the merge queue, why a pull request is not merging | yes (slices 2, 5) | its own (had) | A task delivers its own work |
  | Arm or re-arm a task's merge; merge a task's pull request | yes, for the task's owner (slice 5) | its own, after an independent review (had) | The person decides what of theirs merges, as the page's Merge does; an agent delivers its own reviewed work |
  | Dev deploy, CD status | yes (slice 2) | yes, when its task asks (had) | SPEC-134 |
  | Production deploy and rollback, promotion to `main` | behind the person's typed project slug in the panel (slice 8b); a dev-stage rollback at once | no (a dev-stage deploy and rollback, had) | ADR-0110: a person decides; the Superagent prepares and asks |
  | A task's subscription: see, move; model and effort | yes (slice 6) | sees them in the task list (slice 6) | Tasks are subscription-agnostic (SPEC-001 ROT-007); a person moves their task |
  | The pool's settings: priorities, excluding a subscription | no: no such setting exists ("the pool has no priorities or switches") | no | An administrator adds, signs in or removes a subscription on its panel |
  | Instance: version, health, update status, checks | yes (slice 7) | no | Not a task's work |
  | Instance: Update now, Restart, Cancel, Check | yes, a superadmin's (slice 7; not the Global Superagent) | no | The buttons' rights and refusals |
  | A task's screen, live or its last run's, and how that run ended (exit code, reason) | yes (`task_screen`, slice 10b) | no | The page's terminal shows it to anyone who sees the task; a task's agent reads its own transcript, not another's screen |
  | A task's tokens, the period's by model | yes (`task_usage`, slice 10b; cost as tokens: no price per task is kept) | no | The Dashboard's figures |
  | A task's or the host's load | the host's (`instance`); a task's: no | no | Yulius measures the host and the sessions per sandbox, not each task's processes |
  | How far the repository's checkout is from its integration branch | yes (`checkout`, slice 10b) | it has its own branch and `git` | A stale checkout gave a wrong "no" |
  | Instance: logs, `yulius doctor` | no: no page shows them; `instance` shows health (load, memory, disk, sessions) | no | The logs and the doctor are the host's (`journalctl`, the CLI), outside what Yulius serves |
  | Another sandbox: list and read tasks | yes (slice 8a) | no | SPEC-335: only the person's Superagent reaches their other sandboxes |
  | Another sandbox: create, change, message, stop | behind one confirmation in the panel (had; lasting, amended, withdrawn in slice 8a) | no | ADR-0227, ADR-0333 |
  | Leave a decision only the person makes, answered Yes or No on its panel (`person_ask`) | yes (slice 8a) | asks with `needs_you` (had) | A question in a report is not read; the panel keeps it until answered |
  | A secret's value | never | never | SPEC-002 ADR-20: no agent sees a value |
  | Enter a secret's value | the person, masked, on the Superagent's panel (`secret_request_value`, slice 10a) | no (it asks a person: `secret_request_copy`, `needs_you`) | The value goes from the person to the vault; ADR-0336 |
  | Report a missing ability to the Yulius project | yes (`ability_report`, slice 11) | yes (slice 11) | Both meet walls; ADR-0337 |
  | Standing jobs (monitoring, a disk guard) | yes: scheduled checks Yulius holds (`superagent_check_set`, slice 9b) | its own automation or task | Its session's timers end with every restart; ADR-0335 |
  | Text typed to it while it restarts, or left in its prompt box | kept (slice 9a) | (tasks: SPEC-468, SPEC-480) | A person's message was lost to a restart |

  What its person does on the project's pages it does for them when they
  ask, with their rights and audited as theirs through the Superagent: it continues a task that
  stopped or crashed, pauses and resumes one, starts one that waits, reads a task's delivery
  (its pull request, checks, what comes next and what is refused now and why), reads the
  CI runs and a run's failing output, runs a run again or cancels it, reads the deployments and
  deploys a dev-stage environment. It renames a task, rewrites the request of one that has
  not started (the files attached to it stay attached; never by deleting the task and
  creating it again), puts the person's queued tasks in the order they ask for, closes out an
  idle task they call finished, and pauses, resumes, stops or messages several tasks, or all
  of the person's, in one call. Told to send past text its person left unsent in a task's
  prompt box, it does (the text goes to the agent with the message); nobody else's message
  does. It sees which tasks wait for the person and why without anything pasted to it (what
  a task's agent asked, why a task is flagged, a prompt Yulius held, who paused it; `tasks_list`
  shows the same to a task's agent, without a held prompt's words), reads the end of a
  session in one call, passes the person's answer on, dismisses a request they call settled
  and decides a held prompt as its card does. It says where a task's merge stands and why a
  pull request that passed has not merged yet, lists the project's merge queue and merge
  train, merges a task's pull request for its owner (at once on green, otherwise armed until
  its checks pass) and puts a queued CI run first in the project's queue. It reads the subscriptions and
  which one a task runs on with which model and effort, moves a task to the subscription its
  person names, and sets a task's model or effort for its next start. It reads the instance (its build, an
  update offered, the host's load and disk, the sessions), and for a superadmin checks for,
  installs or cancels an update and restarts Yulius, with the buttons' own refusals. It no longer hands such a job back as steps to click or
  uses a finished task as hands; an ability it lacks it names
  (SPEC-487, [decision 0330](decisions/0330-the-superagent-does-what-its-person-may-do.md)).
  After an update
  that gives it new tools, an open Superagent restarts by itself once it is idle and goes on with
  its conversation; it waits while its person has text in its prompt box they did not send, and
  what they type while it restarts reaches the new session (SPEC-487). A standing job its person
  asks for ("keep monitoring the deploy", "check the disk every hour") is a scheduled check Yulius
  holds, not a timer of its session: due, it is typed into the Superagent when it is idle, after
  any restart, and a Superagent that is not running (its person ended it, the machine restarted)
  is started for it
  ([decision 0335](decisions/0335-the-superagents-standing-jobs-are-held-by-yulius.md)).
- **Global Superagent**: besides the Superagent of each sandbox, every person has one Global
  Superagent, the **All sandboxes** tab of the Superagent panel beside its **This sandbox** tab
  (as Notes shows its notepads; SPEC-428), in every sandbox and on the pages outside one (where
  the rail shows only the Superagent), or at `/superagent`. The tab picked last stays in front
  from sandbox to sandbox, and its note gives the example "check what my sandboxes need from each
  other and solve it". It sees every sandbox the person can see and keeps them in sync: "which of my sandboxes
  have failing CI?", "the API changed in acme/api, make its clients follow" or "tell every running
  task to merge dev" become tasks, pull requests and task messages in each sandbox, as the person,
  without asking per action. It also drives any of those projects' browsers, signing in with their
  agent secrets without seeing them, its cursor shown as "alice's Global Superagent" (SPEC-437).
  The person types into its terminal and clicks in it exactly as in the sandbox Superagent's (SPEC-430), and a screenshot pasted or an image dropped there reaches it as an image (SPEC-433); only they can open it. It has no shell or file tools: it works only through Yulius's tools
  (the remote MCP's, SPEC-247), and deploying, agent secrets, variables and adding or onboarding
  projects stay the person's, and the pull requests it opens are merged by a person or a task. Its conversation is kept in a folder of its own, so it continues
  when it starts again under another sandbox; a person with only read access has none (SPEC-427).
- **MCP servers**: configured once on the MCP page with two settings, Available in (every sandbox
  or this one) and For (the whole team or only me), and attached on every subscription. A team
  server goes to every task run of its scope and to the Superagent; a personal one only to its
  owner's own tasks and Superagent, never to a teammate's or an automation's task, and nobody else
  sees it, so a server holding a person's own credential belongs there (SPEC-332). New servers
  start on every sandbox and the whole team; both settings change later. Their secrets may come
  from the agent secrets. A repository's
  `.mcp.json` servers join its sandbox's list by themselves, with `${VAR}` becoming the agent
  secret VAR's placeholder (the egress proxy swaps the value in on its bound hosts), and the
  Superagent never asks about them. They show "From .mcp.json", and since agents can write that
  file, Test does not run their command on the server until a person changes it (SPEC-218).
- **Sandbox tabs**: the top row starts with All sandboxes, then a tab per sandbox in the order
  the person chose: with a mouse or pen each shows a drag grip, as the Notes tabs do, and each
  says it can be clicked or dragged; a dragged tab stays within the row, giving a little at
  either end; its menu moves it too.
- **Sidebar**: the sandbox pages used every day (Tasks, Git, CI/CD — one page whose CI (tests) and CD (deployment) tabs work like the Secrets page's tabs —, Secrets, Dashboard — with Overview, Pipeline (stages) and Harnesses tabs —; the Superagent is on the right),
  then "More" for Sessions, Workflows, Skills, Automations and MCP, then the active tasks.
- **Tasks**: each task runs a real Claude Code CLI in a PTY on its own git worktree. Sessions
  outlive server restarts. Yulius keeps that CLI current by itself: a new Claude Code release
  (npm's `latest` by default) reaches every new or resumed session within about an hour and a
  running task when its turn ends, with no one signing in again; a release that does not know
  Yulius's default model is never used, and Settings, Instance shows the version, the channel
  and "Check now" (SPEC-421, SPEC-461). A task, Cockpit or Superagent that names no model or effort runs
  Opus 5.5 at high effort (SPEC-023, SPEC-462, SPEC-493); the composer and Settings choose
  another, Fable too. A Claude Code task's independent code review runs as Yulius's reviewer agent, whose
  model Yulius picks per round: Fable 5.2 for a pull request's first two rounds, Opus 5.5 from
  the third, and Opus 5.5 when the task's subscription cannot run Fable then; each round is
  recorded with its reviewer, its model and its verdict. Continuing a reviewer after it reports
  counts as a new round in the Review card (SPEC-565). Codex tasks request GPT-6.2 Astra for every
  review round, and their named review subagents populate the same delivery history and Review
  card. Antigravity tasks request Gemini 4 Neon (High).
  Task intake splits mixed requests, holds scope changes and chains
  dependent tasks, also by a logical rule such as "#12 done and #13 done", which the composer
  builds from picked conditions (a task, its state, all or any). A task that waits for another
  starts when that one has really finished: not in the minute its green pull request waits in
  the merge queue, and not between a merge and the moment its agent hears of it and goes on
  (it reads "Waiting for checks or merge" then, not Done). The tools show a waiting task's rule
  whole, and "after all current tasks" (`after_all_tasks`) waits for every task with work
  still coming (SPEC-487).
  A person’s open Claude goal in a prompt-mode session follows it to another subscription.
  The task stays Working while it waits for the resumed evaluator’s verdict (SPEC-540).
  A task whose turn ended and that waits for a person (Needs you, Done) is **parked** after
  half an hour with nothing going on (`YULIUS_PARK_AFTER_MINUTES`): its agent process ends,
  so it holds neither a session slot nor memory, and it stays in the list where it was, marked
  "parked", with its last terminal screen, idle duration, transcript, question and pull request.
  An open step, unfinished spec or unfinished goal keeps the session live. Writing to it, or Resume, continues
  the same session with its history (SPEC-512). A queued task says what it waits for ("Waiting
  for a free session: 8 of 8 in use"), and the Capacity card shows working, parked, idle and
  queued sessions apart.
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
  after the text). A file can be up to 1 GB by default, so a screen recording of a bug fits: the
  agent extracts frames from it with `ffmpeg` and looks at them (SPEC-325).
  **New draft** opens another unfinished task in the same sandbox. **Drafts** lets a person
  return to a prompt with its attachments and task choices, even after reloading the browser.
  Starting or discarding one draft leaves the others available. [SPEC-594](../.ai/specs/SPEC-594-i-should-be-able-to-create-many-drafts-l.md)
  A task is its owner's: Yulius acts on GitHub for it with their credentials (the agent's own Git
  and `gh` use a narrowed App token), so only they steer it: type into it, send it messages,
  answer what it asks in its terminal, continue, start, pause or deliver it. Anyone who may work in the project
  can **stop, archive, restore and delete** any of its tasks (a playground's guest only their
  own), from the task's page, the lists and the sidebar; stopping or deleting someone else's task asks first and names its owner, and
  the audit says who did it (SPEC-490). Its owner can **pause** a running task and resume it later: the agent
  stops where it is without being told (as an update pauses every task) and goes on from there; a
  paused task, and one an update holds, reads Paused in the lists and says why. A task that
  reads **Needs your action** says why in the bar over its terminal, whenever it is opened:
  what is wrong, what to do, and the page where to do it when Yulius flagged the task itself
  (the Stop gate gave up, no goal was set, a session is full, the agent asked for a secret or
  for an action in the sandbox browser), and "Its question is in the terminal below" only for
  Claude Code's own question; Reply puts the keyboard in the terminal and says so (SPEC-472).
  Others
  watch it as its live transcript (printed as the Sessions view prints a session, in the
  terminal's colours, following its newest lines until they scroll up), see whose it is
  in the view and in the sidebar, answer what its agent asks under Needs you with Reply
  without taking control (SPEC-516: the agent gets the answer labelled with their name; not
  while it asks in its terminal or holds the owner's personal secrets or MCP servers). An agent
  that needs two people names them (SPEC-512: `needs_you` with `who`, each with write access):
  the request says "Asks @bob", only its owner and whom it names answer it, from the page or
  through their own agent's `task_message`, and the owner withdraws a name there; an answer to a
  question that was replaced or withdrawn meanwhile is refused saying so. And they can
  take control, from the task's page or its menu in a
  list, after a confirmation naming the owner (SPEC-510): the task then acts on GitHub as them,
  and the previous owner hears of it wherever they are and can take it back at any time. A
  person who wants to be asked first turns on "Ask me before taking control of my tasks" in
  Settings; others then ask them for control, the owner hears of the request wherever they are
  (a notification, a toast and a mark on the task's row) and hands it over or declines; once
  handed over, it acts as the new owner, and whoever handed it over can take it back at any time
  without asking. A task whose owner can no longer act (a disabled account, no write access)
  passes to the first person who takes or asks.
- **Subscription pool**: Claude accounts are used as one self-managing pool, and Codex and
  Antigravity accounts each as one of their own (SPEC-503, SPEC-505); switches and pauses are invisible to agents. Each task shows the subscription it runs on, and a person can
  run it on another from the task ("Run #N on…"); removing a subscription moves its tasks, and a
  subscription Claude refuses (cancelled, downgraded) is taken out of the pool and says why.
  If a person has already signed it in again, the new sign-in stays available. A
  task that moved for either reason says so in its terminal, and one with nothing usable left is
  under Needs you. A subscription out of a model's usage credits (Fable runs on a weekly
  allowance of them) is at that model's limit: its tasks of that model move on or
  wait for the reset by themselves, the pool sends it no such task until then whatever its
  5-hour and weekly usage read, and nobody is asked to send "continue"; its card shows the
  allowance as "Fable this week", and a task of another model can still be run on it. A limit
  met while a session runs a dynamic workflow (Claude Code's "ultracode") is handled the same
  way: the task moves or waits by itself, and its agent is told to resume the workflow from
  where it stopped, not to start it again (SPEC-486). Every
  card says how old its usage reading is and which window is used up with its reset (the
  5-hour window, the week, or a model's allowance), worked out when it is shown: it stops
  saying "Limit reached" the moment the window resets. An administrator presses Refresh on a
  card, or Refresh all, to read the usage again and to have Claude asked whether a limit it
  reported still holds: after buying credits the limit ends at once and the tasks that wait
  for it go on (SPEC-473). When
  Claude does not say when such a limit ends and no other subscription can take the task,
  Yulius tries it once more after the provider's next usage reading. Refused again, the task
  waits for another subscription's reset when one is known, and otherwise asks a person
  instead of trying on; that request ends by itself once the task's agent works again or
  Yulius takes the task on after all (SPEC-464). A task set to a Fable model does not wait
  while a subscription can run Opus 5.5: when Fable is used up on
  every subscription its session goes on with Opus 5.5, on the same subscription when that is
  the one with room, and says so; the task's details, the Capacity card and the Status line
  say it too, and the task's next launch runs Fable again once a subscription can (SPEC-482).
  A default task and the Superagents run Opus 5.5, which has no fallback: they wait for a
  reset. A session whose person switched it to Fable inside it (`/model`) runs Fable while the
  subscription has credits for it; refused, the limit is recorded as Fable's and the session
  goes on with the model Yulius runs it with (its own, or the fallback of a task set to Fable),
  wherever the pool places it, the same subscription included, and says so (SPEC-493). Its
  Capacity card says what
  is left, how fast it burns, the tokens recorded for its runs and their estimated cost at standard Claude and GPT-6 API
  prices, with a 7-day chart for each. When eligible Codex runs contributed usage, the card also
  shows the weekly total if those runs used Ultrafast. [Decision 0440](decisions/0440-estimate-codex-work-at-standard-and-ultrafast-api-prices.md)
  Administrators rename each subscription in place ("work
  Max", "personal") and see the email of its Claude account wherever it is named; when Claude
  does not share it, they type it in place under the name (SPEC-463). Every card says whose
  subscription it is with a badge under its name (Claude, Codex or Antigravity, the vendor on
  hover; SPEC-513). A subscription is connected
  with Claude's own sign-in in a terminal on the page, or by pasting the token
  `claude setup-token` printed on any computer (its owner's laptop, say); a card's "Paste a new
  token" signs one in again the same way (SPEC-502). Connecting one
  is the first step on a fresh instance: until a Claude account is usable, Home, the composer and
  the onboarding gate show Connect Claude, and every start a person asks for is refused at once
  with a link to Subscriptions; tasks started by a chain, a rule or an automation wait and say why
  (SPEC-246).
- **Delivery discipline**: spec when it matters (a task that needs one is refused code edits with
  Claude Code's edit tools until its spec has a Plan, SPEC-426; Bash edits and the spec's validity
  are the turn-end gate's),
  effort-weighted steps that hold all the work the agent knows of (a pull request delivers the
  finished part, and a plan that grew says how many steps it had at first, SPEC-484),
  independent review
  (agents fix findings in their own code and obtain re-review through approval under the task's existing authorization),
  merge on green into `dev`, cleanup; promotion to `main` is a person's. Profiles live in
  `yulius.toml`, with the branch policy (typed branches, conventional titles, `dev` → `main`
  promotions) that Yulius checks before every merge. Each task records what it delivered in the
  project's changes log, or, with `[docs] changes = "docs/changes/"` (a trailing `/` or an
  existing directory), in one new note of its own in that folder, named
  `YYYY-MM-DD-<task number>-<short-slug>.md`, so parallel pull requests never conflict on it; the
  briefing then quotes the newest notes as Recent changes (SPEC-363).
- **Post-merge verification** (SPEC-562). A task's Delivery links to the CI job verdicts and environment attempts for its landed change. Projects that accept task evidence can close a machine-verification step by citing the checked attempts in its note. [Git workflow](engineering/git.md#post-merge-machine-evidence)
- **Merge queue** (every sandbox, SPEC-441): a pull request's own CI run is never restarted
  because the integration branch moved; armed and green, it merges at once when up to date, else
  Yulius tests it once on top of the current integration branch, together with the others
  waiting (a merge candidate, halved when red), and merges them in order as soon as that run
  passed, without waiting for their own runs (SPEC-497). If a candidate rebuilds a tree that
  recently failed in this queue, bisection uses the original run (SPEC-542). The Pipeline tab says why each recent
  candidate was dissolved. Every armed pull
  request says where it stands, and a merge at a head whose own Yulius CI run passed counts as
  merged on green, whoever made it.
- **Pull request notices** (every sandbox, SPEC-444): Yulius types what happens to a task's open
  pull request into its session within a minute, with the next step: a red run (failing jobs and
  tests, partial failures observed in stopped siblings, where to read the logs), a conflict, a dropped or refused merge, the merge, why a green
  head is not merging, and "Hands off" while a merge candidate holds it. A quiet session gets a
  check-in, a pull request without progress for an hour is told and shown as stalled on the task
  list, `yulius wait` never ends on a fixed limit while its run is under way, and merging by hand
  is refused with the reason.
- **Yulius CI**: the projects' only CI; it runs on the VM, no GitHub Actions. A job uses an agent
  secret only when the integration branch's `[[ci.jobs]]` entry names it (`secrets`) and the job
  runs that entry's command (a pull request's own job with another command gets none), as a
  placeholder through the egress proxy toward the secret's bound hosts, audited as `ci:<run>`;
  every pull request of a project that a Vidoc key reaches, with its scans on, runs Yulius's own
  `security` job, granted `VIDOC_API_KEY`, which scans the pull request's changes with Vidoc's
  vidoc-1. A job that checked nothing exits with
  `$YULIUS_CI_SKIP_CODE` and reads **Skipped**, never Passed; it blocks no merge (ADR-0133).
  A run opens in place of the CI/CD page's list, at its own address, so Back and the sidebar come
  back to it; its summary stays on top with the time so far and the stages running now. Above the
  tabs the Now summary lists what runs, CI and deploys, with its time, one click from each
  (SPEC-238, ADR-0180).
- **Vidoc security scan** (SPEC-195, ADR-0157): an optional instance key, which the superadmin sets,
  tests and removes like the Jev key, and which setup offers as a skippable step. With it, every
  project's agents and granted CI jobs run `yulius security-scan`: Vidoc's vidoc-1 reviews the
  branch's changed files (or, with `--all`, the whole repository). Open critical or high findings
  fail the scan, and so do open medium findings in the lines the branch changed; the agent decides
  on low ones there (SPEC-217, ADR-0172). Findings in code a pull request did not touch become
  GitHub issues, filed once, reopened while reported and closed once the code is gone. Every project
  with CI whose sandbox has a Vidoc key runs the scan as a built-in `security` job, unless its CI
  already scans, it is a playground or its scans are switched off. The code goes to
  Vidoc, but the key never reaches agents: the egress proxy adds it toward
  `model.vidocsecurity.com` only. With a key, infra bootstrap adds only the AGENTS.md rule to scan
  before delivery, never a job of its own. Without one, the scan reports "not run". A person can
  switch a project's scans off, for example while the key's quota is used up, on the Instance
  page's Vidoc card or when adding the project ("Scan pull requests with Vidoc"); it then scans
  nothing, its scans say "disabled", Yulius adds no `security` job (one the repository configures
  itself ends Skipped), and its agents ask nobody about the key
  (SPEC-338, ADR-0229). The sandbox's own admins switch the same setting on its CI/CD page (the
  Security scan card), which also says when the repository's committed `.config/vidoc.toml` or
  the instance has Vidoc off and who changed it last; a scan whose key is refused or out of quota
  names that page (SPEC-359, ADR-0245). A superadmin can switch Vidoc off for the whole instance on the Instance
  page's Vidoc card ("Scan every project with Vidoc"; the Security scan card shows it), keeping the key, also one from the server's configuration
  file: every project then scans nothing in the same way until it is switched on again, and each
  project keeps its own choice for then (SPEC-356, ADR-0242).
- **Pipeline (stages)**: every stage of tasks, CI runs and deployments is measured as it happens
  and kept 180 days (SPEC-137): a task's waits, states, steps (with their tokens), runs, pauses,
  pull request and merge waits; a CI run's queue (and why), jobs and the stages their output names
  (`::yulius-stage`, with test counts); a deploy's confirmation, promotion, deploy and verify; what a
  CI job or deploy used (CPU time, peak memory, bytes written; SPEC-154). Each task keeps its
  latest completed merge wait beyond the telemetry window (SPEC-558). The Dashboard's Pipeline
  (stages) tab (`?section=pipeline`; `/pipeline` still opens it) shows deployment frequency, lead
  time, change failure rate and time to restore for dev and production, any stage's daily p50 and
  p90 and the stage table (with CPU and memory p50 · p90) with CSV/JSON export; a CI run, a deploy
  and a task show their own waterfall (a time axis, a legend, children folded under their span; a
  CI run's jobs one row each with their wait for a job slot, a job still waiting included), and
  the Dashboard's Overview tab the overview.
- **CD (deployment)**: the CI/CD page's second tab, with CI's discipline. Every project defines dev and
  production in `yulius.toml` (`[cd.dev]`, `[cd.prod]`), and one more `[cd.<name>]` per further
  target (a staging machine, a hosted platform next to a self-hosted release) with the `stage`
  whose rules it follows, each with deploy and verify scripts
  committed under `scripts/cd/`, or explicitly empty while it deploys nothing yet. Yulius runs the
  exact commits GitHub reports for the protected branches, dev inside the sandbox and production
  outside it, with the agent secrets allowed for each (see the secrets vault below), and shows each run on the CD
  (deployment) page; a confirmed deploy shows its run's view at once, saying what it checks and
  promotes, and that view becomes the run. A merge deploys nothing by itself: a person or an
  agent (`cd_deploy`) deploys dev when it is wanted, or the ready-made "Deploy dev every night"
  automation deploys and tests it.
  - **Merge-first staging** (SPEC-575) applies to a project whose shared staging target is
    `merge_first = true`.
    - Pull requests merge on the quick checks. The full suite (`after_merge` jobs) runs once on
      the merged revision.
    - A task asks for staging with `cd_deploy` once its delivery merged. Yulius deploys and
      verifies that tested revision in revision order, once per task. A running task with an unfinished step shows the full-suite
      or staging wait. Finished tasks keep the outcome on their delivery. Later deliveries
      record inherited failures and complete when staging serves the verified fix.
    - Working tasks hear the failing tests and their introducing merges. One worker takes the repair.
      With no worker available, the project CI and Status APIs return the unresolved tests and the run a writer can retry.
    - A person can deploy the revision staging verified while the next dev revision is being checked. [ADR-0425](decisions/0425-pin-a-production-deploy-to-verified-dev.md)
  - Each task tests locally first with the project's `[local]` deployment inside its own sandbox.
  - New projects with such a target are bootstrapped this way, and onboarded ones get one task
    that adopts it.

  Every environment that deploys also defines its rollback
  (SPEC-415): a command that redeploys a release Yulius kept from an earlier deploy, never
  building, and a line saying how data changes stay compatible. The CD page lists each part's
  three newest releases (older ones fold), each with one action: "Roll back to previous" on the
  previous one, "Roll back to this" on the others (agents roll dev back with
  `cd_rollback`), refuses rolling back past a release whose migration cannot be undone and says
  why, drills the rollback on dev after every deploy, and refuses a production deploy until that
  drill passed for the commit it deploys. When only that drill is missing, Deploy production
  deploys dev at the commit first, waits for its drill and then deploys production by itself, or
  says why production did not deploy (SPEC-438). An environment that runs no deploy command (an empty one,
  or a production that only promotes, as Yulius's own) needs no rollback, and the page says so and
  why; Yulius itself rolls back through its update, which restores the previous binary and database
  when the new build fails to start (its health check), or by a revert promoted again (SPEC-418).
  Only where a delivery profile promotes by itself (`auto_promote`, the Studio preset) does Yulius
  promote, once each merged change passed its integration environments; a production deploy stays a
  person's there too. Production (with its promotion) is otherwise always a person's: an
  administrator types the project's slug, and on GitHub a ruleset only repository admins bypass
  keeps the production branch out of reach of the agents' own token, which is never a person's
  (onboarding says when that ruleset is missing, and its agent adds it with `production_protect`
  as the person who started onboarding when they are a repository admin; where the GitHub plan
  has no rulesets, a sandbox admin waives it on the Git page's Production protection card, and
  every request for the ruleset says so). A task that needs to read
  another repository of the same GitHub owner gets it with `reference_repo`, fetched as the task's
  owner with every branch and tag that person can read (SPEC-330), instead of asking a person for
  access; onboarding fetches the repositories a project
  refers to first (SPEC-267). Agents build and improve CD through pull requests but
  cannot change what a deploy runs, and they ask a person with `needs_you` for what only a person
  can give (a machine's credentials). Onboarding finds how a project deploys today and writes it
  down this way; "Set up CD with an agent" does the same later, through a task. For Yulius itself
  dev is empty until it has a dev machine, and production is the push to `main`, which each
  instance installs once a superadmin chooses Update now.
- **Browser panel**: the sandbox's real Chrome streamed as video so people can pass captchas
  and sign-ins for the agent. Their clicks and keys need the host's browser policy, which
  `yulius host setup` writes: without it the browser shows but takes none (ADR-0318).
  After clicking a webpage’s Copy button on the virtual display, choose **Copy to my clipboard** to transfer plain text from the shared sandbox browser to your device.
  If the policy arrives after an agent used the browser, the panel offers **Restart browser** to recover input while keeping that sandbox’s profile.
  New browser launches keep Chrome’s default extension prompts off the sandbox display,
  including for existing profiles (SPEC-567).
- **Automations**: tasks on a schedule. Three ship with Yulius and head the Automations page as
  its Defaults (as the default workflows do), each off until a person switches it on: "Keep the
  docs up to date", "Work through open issues" (the issues of the core team, of people with write
  access and of this project's agents, ranked, and the 5 most important actionable ones that no
  running task owns verified in parallel and given a task each, no new issues) and
  "Deploy dev every night" (a CD run of dev, tested, a failure filed as an issue),
  nightly at 03:00 CEST. An automation's tasks start like New task's: now, as a supertask, after
  another task or when a rule is true, or wait prepared until the person presses Start now ("When
  I decide"), picked with New task's start chip beside Run now and Save;
  Run now starts its task the way the chip shows. Each task runs as the automation creator.
  Only the creator can save changes, toggle the schedule or use Run now. Project writers can
  delete the automation and create a replacement of their own. [ADR-0381](decisions/0381-automation-creators-authorize-edits-and-starts.md)
- **Notes**: in the tools rail, a notepad per sandbox and one for the whole team, shared and saved
  while people type; a teammate's edit shows up live, and two edits at once end in a choice. A
  third, **Me**, is each person's own: nobody else sees it, and it is the same in every sandbox.
  The tabs go in any order (Team first until moved); each shows a drag grip and says it can be
  clicked or dragged. Selections are drawn rounded and glide, and
  teammates who have a shared notepad open show as a caret with their name and their selection
  in their own colour.
- **Editor**: Monaco over the sandbox's files for people with write access. Like Notes, its
  selection is drawn rounded and glides, people who have the same file open edit one shared
  text and see each other's keystrokes, caret, name and selection in their Notes colour before
  anything is saved, and its tabs go in any order. It saves by itself about a second after typing
  stops and when the person leaves the file (SPEC-411). The file in front follows the disk: what
  agents and shells write shows within about 2 s; over unsaved text every Editor offers Keep mine
  or Take disk version (SPEC-181, SPEC-411). Its folder picker opens any branch the sandbox has checked out, named by its branch:
  the sandbox repository or any task's worktree, also with no task selected; another person's
  worktree opens read-only and says why (SPEC-340).
- **Editor, terminals, Git view, automations, workflow catalog** in one web UI, whose menus fit
  the window and filter as people type when they are long; the start menu shows five tasks at a
  time and says how many more it scrolls to. Scrolling a task's terminal past its top offers the
  whole session from its first message, which Claude Code no longer shows after a compaction
  (SPEC-326). A wheel gesture on a full-screen task with mouse reporting off leaves the prompt alone
  (SPEC-529). In a Codex terminal, right-click and choose Paste to insert clipboard text in the draft
  (SPEC-582). Messages are
  withdrawn for now (SPEC-028): agents learn about checks, delivery and armed merges from
  `yulius wait`, `ci_status` and `delivery_status`. A Yulius restart is invisible to a session:
  its Yulius tools and `yulius wait` hold on until Yulius is back, and a task that went idle
  meanwhile is told "Yulius is back; continue" (SPEC-358). An agent removes the files its task created
  with `yulius remove` (SPEC-192). A contained Claude reviewer whose scratch cleanup uses
  a variable or names a root-level scratch file receives a checked retry directly. [SPEC-596](../.ai/specs/SPEC-596-such-requests-should-be-answered-automat.md)
- **Agent primitives as code**: Yulius's own skills are APM packages with a lockfile, deployed to
  every harness and checked for drift; onboarding codifies a project's conventions as primitives.
- **Secrets vault**: secrets are a title, a value and notes. Team secrets are for people; agent
  secrets are used by agents, who never see them. Reading a value needs your own two-factor code
  (set up once, mandatory); an administrator resets it for someone who lost their authenticator.
  On the Secrets page, managers see which older names set interpreter options and can rename them. Host-bound GitHub tokens remain supported. [ADR-0408](decisions/0408-interpreter-settings-are-reserved-for-new-agent-secret-names.md)
  The people who manage an agent secret can read it too. Sending a value somewhere new reads
  it: for an agent secret allowed use by use, a new host, `.env` copies, production deploys and
  letting agents set it up ask for the same code, and an API token cannot do them (ADR-0317). What cannot be checked
  for secrets is withheld: a terminal says so and shows nothing, the proxy sends nothing, until
  the secrets can be read again. A shown value stays masked until its eye
  button is pressed; it copies without being shown, from a copy icon always in its box, as the
  notes do (SPEC-207). A multi-line value (a key, recovery codes) pasted into a team secret keeps
  its line breaks as pasted, masked, and shows and copies on its lines (SPEC-299). An agent secret
  is added with a name, a value and a description: agents find the service's host and bind it
  themselves, and production deploys get the secret, unless the person turns "Let agents set it up"
  off when adding it; then a person allows each host with one click and production separately
  (SPEC-121, SPEC-216). Team secrets belong to the sandbox they were added in, or are team-wide when
  the person picks that; team secrets from before were copied into each sandbox (SPEC-216). Agent
  secrets ("keys and logins agents use without seeing them") are on three tabs (SPEC-245):
  **Project** (this project's), **Global** (defined once by instance administrators, who switch each
  on per project — e.g. a token for private packages many repositories need) and **Personal** (one
  person's own, switched on per project by them, used only by tasks they start: never by other
  people's tasks, automations, CI or deploys, never seen by anyone else, and sent only by the proxy;
  co-writers who can get code run in the person's tasks are trusted, ADR-0186). A name
  resolves to the project's secret, else the person's, else the global one; a card says when
  another wins. A project's keys live in a git-ignored
  `.env` beside a tracked `.env.example`; when its code needs a secret's real value, the agent asks Yulius to copy it
  there, and a person allows that once per secret, unless it was imported with `--env-files`
  (SPEC-129, SPEC-256). A multi-line key (PEM, OpenSSH) is
  pasted into the value as it is: the field keeps a key's line breaks as literal `\n`, and it lands
  in the `.env` as its file, byte for byte (SPEC-186, SPEC-193); Show and Copy value give that
  file too, with its line breaks (SPEC-431). A key saved before that, its line
  breaks lost, is marked "Paste again" for the people who manage it. From a computer, `yulius
  secrets import --to team|agent [file]` brings in a `.env` (or typed, hidden values) at once,
  into the checkout's sandbox, or with `--instance` team-wide (team secrets) or global, switched on
  for that sandbox (agent secrets):
  every name is listed first, nothing that exists is overwritten, nothing is sent before the
  person types `yes` (or, for their own agent, `--yes` with its operate token, SPEC-256), and no value is ever shown, passed as an argument or written down
  (SPEC-205). Values that are ordinary text (configuration words such as `production`, host
  names, e-mail addresses, URLs without credentials), which would mask that text everywhere as
  `[secret:NAME]`, are skipped and named; values found in the project's tracked files are asked
  about, or skipped with `--yes`; and the person's agent deletes a secret imported by mistake
  with `yulius secrets delete NAME` (SPEC-274). Agents add one themselves from a file they can reach (a key they generated, a key in
  `~/.ssh`): Yulius reads the file, so no one sees the value. The new secret has no hosts and no
  `.env` allowance until a person allows them, an existing name is never replaced, and the
  Superagent asks before it does this on an uncontained server (SPEC-211). Agents never use a
  production secret; when their work needs the same value (a dev CI token that exists only as a
  production secret), they ask for a copy with its name and hosts, and a person allows it on the
  production secret's card with **Allow copy** and their two-factor code: Yulius adds the copy
  from the stored value, so nobody reveals or pastes it (SPEC-440).
- **Updates**: Yulius updates itself from `main` without losing running work. The superadmin
  starts an update from the Update button beside their account button in the sidebar (SPEC-133).
  Once the new version is built, every task is frozen at once, mid-turn, and thawed after the
  restart; the agents don't notice (SPEC-160). A deploy (CD or environment run) running when the
  new version is built finishes first, and new deploys are refused until the update installed, so an
  update never cuts a deploy (SPEC-342). A planned install gives a CI gate with passed work and only its last job left up to 30 seconds from the start of the task pause. **Continue without waiting** skips this wait (SPEC-573). **Restart…** on the Instance page restarts the server
  the same way without installing anything (SPEC-432). Sessions live in a systemd unit of their
  own, so the restart keeps them on every systemd; on a host not set up for that, where the
  server could not start again while a session runs (systemd 258 and later), Restart and Update
  now are refused with the fix, `sudo yulius host setup`, instead of taking the instance down
  (SPEC-458). A task session whose Yulius tools Claude
  Code dropped is reconnected by Yulius, and an agent can call any Yulius tool from its shell with
  `yulius tool` (SPEC-432).
- **Notifications on a person's phone** (SPEC-498, ADR-0351, part 1 of #1107): an instance
  administrator sets one Telegram bot for the instance in Settings → Administration (checked with
  Telegram, stored encrypted, never shown again). Each person links their own chat in Settings →
  Notifications with a single-use, 10-minute code (only from the browser, never with an API
  token), chooses what reaches it (Needs you by default; Done; failed or stopped; subscription
  problems for administrators) and unlinks there or with `/stop`. Only what their notification
  center would show them is sent (a subscription switch is no news, a project's Docker trouble is
  one message however many tasks ask, a request for control of their task, or a teammate taking it
  or taking it back, reaches them under Needs you), with a link to the task, agent secrets
  removed, at most five messages a minute with the rest in one summary. The server asks Telegram
  for messages, so no inbound port is needed.
- **Work from Telegram** (SPEC-512 part 4, ADR-0392, part 2 of #1107), off by default: a person
  turns it on for their own chat in Settings → Notifications and picks the project new messages
  start tasks in. A reply to a notification then answers its task as their message (the owner
  rule holds: someone else's task takes only an answer it asks of them, and only to the question
  the notice told of); a new message (not a reply), with photos, documents or voice notes up to
  20 MB, starts a task as them, marked "From Telegram", at most 20 an hour in a project as for
  agents' tools; `/task <project> <text>` picks another project and `/status` lists what waits
  for them. A chat's messages are handled one at a time in the order sent; with more than 20
  waiting, the chat is told the latest were not handled and to send them again. Turning intake
  off or `/stop` also stops messages still waiting.
- **Tasks hand work to another sandbox** (SPEC-512 part 5, ADR-0395, #1106), off by default:
  an instance administrator chooses Ask or Automatic in Settings → Administration (with a chain's
  depth and a daily bound per owner), and a sandbox's writers can close it to handoffs on its Git
  page. A task's agent then hands the part of its work that belongs elsewhere to another sandbox
  its owner can work in: in Ask mode the owner approves it on the task's page, in Automatic it
  starts at once as the owner. The new task shows where it came from, the source task is told
  when it is done or merged, and both sandboxes' audit logs say so.
- **An end-of-day digest** (SPEC-512 part 3, ADR-0380, #1413), off by default: an instance
  administrator turns it on in Settings → Administration with a time (UTC) and, if they want, an
  idle threshold. Once a day each person with something to report gets what waits for them first,
  what was parked since the last digest with the end of its last message, and how many tasks
  still work, under the bell (it opens `/digest`, also for a digest made while no page was open)
  and in their Telegram chat unless they turned it off there. With a threshold it first parks the
  quiet tasks that wait for their person's turn and ask nothing; Done tasks and tasks that wait
  for a decision are never parked by it.

## Principles

Agents are real CLIs; sessions outlive the server; GitHub is the authority for permissions (a playground's guests, let
in through an expiring link, are the one exception);
coordination goes through one MCP and hook protocol; every queue, stream, retry and wait is
bounded and owned; features have specs, research and questions do not; a defect a request
reports is fixed in its task, also when it is asked as a question, never filed and left
([decision 0371](decisions/0371-a-defect-a-request-reports-is-fixed-also-when-asked-as-a-question.md)).

## Where to read more

- Architecture: [architecture.md](architecture.md); decisions: [decisions](decisions/README.md).
- Requirements and evidence: [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md),
  [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md),
  [SPEC-003](../.ai/specs/SPEC-003-traceability.md).
- Operations: [operations.md](operations.md); API: [api.md](api.md).
- Change history: [changes.md](changes.md).
