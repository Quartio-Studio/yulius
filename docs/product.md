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
  most connections, notified once until it has room again (SPEC-361). Removing a sandbox asks
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
  update helper, Caddy, ufw), copies the data while the old instance is frozen, proves the copy
  (entries, bytes and a fingerprint per folder, equal on both hosts), starts the new instance,
  redirects the old URL and removes its own key. People are asked only in the plan, before
  anything changes (the SSH sign-in, a DNS record, the GitHub App's callback). Any failure before
  the new instance runs thaws the old one; `--abort` goes back. Tasks that were live come back by
  themselves on the new host. Still to come: a warm copy so the freeze sends only the changes,
  the GitHub App check and the button in Settings → Instance.
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
  `/clear` starts over, it cannot be
  deleted and comes back with its conversation. When it cannot reach Yulius from its sandbox, its
  panel says so and Restart Superagent starts it again in place, keeping its conversation
  (SPEC-355). On a contained server it runs in Claude Code's
  bypass-permissions mode, as tasks do, not auto mode (SPEC-208). It manages the project for its owner: asked to
  "create a task from that" or to do something "every day at 4am", it creates the task or the
  automation through Yulius's tools, and it changes how a waiting task starts, stops, archives or
  deletes tasks, runs or deletes automations and enables the ready-made ones (asking the person
  before each change only on an uncontained server). A deleted task's attachments stay while
  another task's request or an automation's prompt names them and otherwise wait 7 days in a trash it restores them from
  (SPEC-396). Task agents
  can do the same for their task's owner, except to their own task. Told "tell #5 it may use the
  DO budget", the Superagent passes the message on (task agents cannot): a task that has not
  started gets it in its request, a running one gets it typed into its session as "Message from
  alice (through their Superagent)" (or "(through their own agent)" when the person's own agent
  sends it through the remote MCP, SPEC-322), unless someone typed there in the last minute or
  the task still asks a person whether to trust its folder (SPEC-230). When a usage limit cuts its
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
  options, and nothing reaches that sandbox until the person presses Create task there; Decline,
  no answer in 4 minutes or interrupting the Superagent creates nothing, and no API token can
  approve (SPEC-335). After an update
  that gives it new tools, an open Superagent restarts by itself once it is idle and goes on with
  its conversation.
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
- **Sidebar**: the sandbox pages used every day (Tasks, Git, CI/CD — one page whose CI (tests) and CD (deployment) tabs work like the Secrets page's tabs —, Secrets, Dashboard — with Overview and Pipeline (stages) tabs —; the Superagent is on the right),
  then "More" for Sessions, Workflows, Skills, Automations and MCP, then the active tasks.
- **Tasks**: each task runs a real Claude Code CLI in a PTY on its own git worktree. Sessions
  outlive server restarts. Yulius keeps that CLI current by itself: a new Claude Code release
  (npm's `latest` by default) reaches every new or resumed session within about an hour and a
  running task when its turn ends, with no one signing in again; a release that does not know
  Yulius's default model is never used, and Settings, Instance shows the version, the channel
  and "Check now" (SPEC-421, SPEC-461). A task, Cockpit or Superagent that names no model or effort runs
  Fable 5.2 at high effort (SPEC-023, SPEC-462); the composer and Settings choose another.
  Task intake splits mixed requests, holds scope changes and chains
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
  after the text). A file can be up to 1 GB by default, so a screen recording of a bug fits: the
  agent extracts frames from it with `ffmpeg` and looks at them (SPEC-325).
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
- **Subscription pool**: Claude accounts are used as one self-managing pool; switches and
  pauses are invisible to agents. Each task shows the subscription it runs on, and a person can
  run it on another from the task ("Run #N on…"); removing a subscription moves its tasks, and a
  subscription Claude refuses (cancelled, downgraded) is taken out of the pool and says why. A
  task that moved for either reason says so in its terminal, and one with nothing usable left is
  under Needs you. Its Capacity card says what
  is left, how fast it burns, the tokens it used and what they would cost at Claude API list
  prices, with a 7-day chart for each. Administrators rename each subscription in place ("work
  Max", "personal") and see the email of its Claude account wherever it is named; when Claude
  does not share it, they type it in place under the name (SPEC-463). Connecting one
  is the first step on a fresh instance: until a Claude account is usable, Home, the composer and
  the onboarding gate show Connect Claude, and every start a person asks for is refused at once
  with a link to Subscriptions; tasks started by a chain, a rule or an automation wait and say why
  (SPEC-246).
- **Delivery discipline**: spec when it matters (a task that needs one is refused code edits with
  Claude Code's edit tools until its spec has a Plan, SPEC-426; Bash edits and the spec's validity
  are the turn-end gate's),
  effort-weighted steps, independent review
  (findings in the change's own code are fixed before merge, never deferred to an issue),
  merge on green into `dev`, cleanup; promotion to `main` is a person's. Profiles live in
  `yulius.toml`, with the branch policy (typed branches, conventional titles, `dev` → `main`
  promotions) that Yulius checks before every merge. Each task records what it delivered in the
  project's changes log, or, with `[docs] changes = "docs/changes/"` (a trailing `/` or an
  existing directory), in one new note of its own in that folder, named
  `YYYY-MM-DD-<task number>-<short-slug>.md`, so parallel pull requests never conflict on it; the
  briefing then quotes the newest notes as Recent changes (SPEC-363).
- **Merge queue** (every sandbox, SPEC-441): a pull request's own CI run always finishes, however
  often the integration branch moves; armed and green, it merges at once when up to date, else
  Yulius tests it once on top of the current integration branch, together with the others
  waiting (a merge candidate, halved when red), and merges them in order. Every armed pull
  request says where it stands, and a merge at a head whose own Yulius CI run passed counts as
  merged on green, whoever made it.
- **Pull request notices** (every sandbox, SPEC-444): Yulius types what happens to a task's open
  pull request into its session within a minute, with the next step: a red run (failing jobs and
  tests, where to read the log), a conflict, a dropped or refused merge, the merge, why a green
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
  CI job or deploy used (CPU time, peak memory, bytes written; SPEC-154). The Dashboard's Pipeline
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
  promotes, and that view becomes the run. A merge deploys nothing: a person or an agent
  (`cd_deploy`) deploys dev when it is wanted, or the ready-made "Deploy dev every night"
  automation deploys and tests it. Every environment that deploys also defines its rollback
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
  Production (with its promotion) is always a person's: an administrator types the project's slug,
  and on GitHub a ruleset only repository admins bypass keeps the production branch out of reach of
  the agents' own token, which is never a person's
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
  and sign-ins for the agent.
- **Automations**: tasks on a schedule. Three ship with Yulius and head the Automations page as
  its Defaults (as the default workflows do), each off until a person switches it on: "Keep the
  docs up to date", "Work through open issues" (the issues of the core team, of people with write
  access and of this project's agents, ranked, and the 5 most important actionable ones that no
  running task owns verified in parallel and given a task each, no new issues) and
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
  (SPEC-326). Messages are
  withdrawn for now (SPEC-028): agents learn about checks, delivery and armed merges from
  `yulius wait`, `ci_status` and `delivery_status`. A Yulius restart is invisible to a session:
  its Yulius tools and `yulius wait` hold on until Yulius is back, and a task that went idle
  meanwhile is told "Yulius is back; continue" (SPEC-358). An agent removes the files its task created
  with `yulius remove` instead of asking a person when Claude Code stops an `rm` (SPEC-192).
- **Agent primitives as code**: Yulius's own skills are APM packages with a lockfile, deployed to
  every harness and checked for drift; onboarding codifies a project's conventions as primitives.
- **Secrets vault**: secrets are a title, a value and notes. Team secrets are for people; agent
  secrets are used by agents, who never see them. Reading a value needs your own two-factor code
  (set up once, mandatory); an administrator resets it for someone who lost their authenticator.
  The people who manage an agent secret can read it too. A shown value stays masked until its eye
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
  update never cuts a deploy (SPEC-342). **Restart…** on the Instance page restarts the server
  the same way without installing anything (SPEC-432). Sessions live in a systemd unit of their
  own, so the restart keeps them on every systemd; on a host not set up for that, where the
  server could not start again while a session runs (systemd 258 and later), Restart and Update
  now are refused with the fix, `sudo yulius host setup`, instead of taking the instance down
  (SPEC-458). A task session whose Yulius tools Claude
  Code dropped is reconnected by Yulius, and an agent can call any Yulius tool from its shell with
  `yulius tool` (SPEC-432).

## Principles

Agents are real CLIs; sessions outlive the server; GitHub is the authority for permissions (a playground's guests, let
in through an expiring link, are the one exception);
coordination goes through one MCP and hook protocol; every queue, stream, retry and wait is
bounded and owned; features have specs, research and questions do not.

## Where to read more

- Architecture: [architecture.md](architecture.md); decisions: [decisions](decisions/README.md).
- Requirements and evidence: [SPEC-001](../.ai/specs/SPEC-001-yulius-product.md),
  [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md),
  [SPEC-003](../.ai/specs/SPEC-003-traceability.md).
- Operations: [operations.md](operations.md); API: [api.md](api.md).
- Change history: [changes.md](changes.md).
