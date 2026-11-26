# Product summary

Yulius is a self-hosted workspace in which a small team runs coding agents on one Linux server.
Each task is a real agent CLI session (Claude Code, Codex or Antigravity, on the team's own
subscriptions) working on its own git worktree inside a contained sandbox, one sandbox per project.
Yulius holds every task to one engineering discipline: a spec when the change needs one, a plan of
steps, an independent review, a merge on green through Yulius's own CI and merge queue, and
deployment through Yulius's own CD, where production stays a person's decision. People watch, steer
and unblock the agents from a web UI, their own Superagent, Telegram or the agent on their laptop.
Agents never see a secret's value: an egress proxy adds it only toward the hosts it is allowed for.
Sessions outlive server restarts and updates. Yulius is written in Rust and develops itself with
Yulius.

## Who it is for

A small team of about 2-5 people who trust each other's agents, works in many repositories and
wants agents to deliver on their own while people watch, steer and unblock them. The instance runs
on one server, the founders are its superadmins, and GitHub decides who may do what in a project.
The team brings its own Claude, Codex (OpenAI) and Antigravity (Google) subscriptions.

## What it does

### Projects and sandboxes

- **Three ways in.** Import a GitHub repository, start a playground without GitHub, or bring a
  laptop checkout and its Claude Code sessions with `yulius onboard`.
- **A sandbox per project**: a contained environment with its own system layer, home, Docker
  engine (`docker build`, `run` and `compose` work for agents, shells and CI jobs), shells per
  person, a real Chrome and an editor. Production deploys get a Docker engine of their own that no
  agent reaches. [Isolation model](architecture.md#process-and-isolation-model)
- **Onboarding is mandatory** for a repository. A person starts it with Start onboarding on the
  sandbox page (or their own agent with an operate token); until it finishes, the new-task page
  shows only what it will do. The onboarding task adapts the repository to Yulius's standards,
  finds how the project builds and deploys, fetches the repositories it refers to, and lays the
  project's gate in its own pull request (tests, lint and format, a dead-code and
  unused-dependency check, a Yulius CI job), with existing findings baselined.
- **Playgrounds** let a team try Yulius on its own code before connecting GitHub: drop a folder (a
  repository keeps its history) or pick a template. Git stays on the server, nothing is pushed, and
  tasks merge into the playground's own `dev` (on green CI when it has CI). Its admins invite
  teammates without GitHub through a link that lasts 1, 7 or 30 days; a guest works on that
  playground only, and its runs then get no agent secret. "Move to GitHub…" pushes it into an empty
  repository, and it becomes an ordinary sandbox with the same tasks and history.
- **Other repositories.** A task reads another repository of the same GitHub owner with
  `reference_repo`, fetched with its owner's access, instead of asking a person.
- **Disk.** Files nothing needs any more (a removed sandbox's worktrees, a done task's build cache,
  build output unused for two days) are deleted by themselves; the superadmin reviews the rest in
  Settings → Instance → Leftovers or with `yulius leftovers`. Removing a sandbox asks whether to
  delete its files or keep them. When disk runs low, Yulius sweeps harder and every page warns.

### Tasks and the composer

- **One harness per task, for life.** A task runs Claude Code, Codex or Antigravity. With nothing
  chosen it runs Claude Code on Opus 5.5 at High effort (composer without a saved choice, CLI,
  Superagents, Telegram). A `gpt-*` model picks Codex. **Random** draws uniformly among the
  harnesses that have a usable subscription; an automation runs the agent it names or draws for
  each run. A follow-up an agent creates keeps its own harness.
- **Models and effort.** The composer and Settings offer the Claude, Codex and Gemini models grouped
  by provider, each with its effort levels, and remember the person's choice. Codex defaults to
  GPT-6.2 Astra at High effort (Ultrafast on ChatGPT Pro 500); Antigravity to Gemini 4 Neon at
  High effort.
- **Attachments.** Images and files pasted or dropped into a prompt sit in the text as placeholders
  (`Image 1`) and reach the agent at that place; a file can be up to 1 GB, so a screen recording
  fits (the agent extracts frames with `ffmpeg`). A person keeps several **drafts** per sandbox,
  each with its text, attachments and choices, across reloads.
- **How a task starts**: now; after another task; when a rule over other tasks holds ("#12 done and
  #13 done", built from picked conditions); after all current tasks; prepared, until the person
  presses Start now or at a chosen time; or as a **supertask** for broad work, which waits for the
  project's running tasks and then runs alone. How a waiting task starts can still change. A task
  that waits for another starts only when that one has really finished, not while its green pull
  request waits in the merge queue. The Tasks page's **Graph** shows the queue, chains, rules and
  holds.
- **Intake** (Jev, with the optional instance key) splits mixed requests, holds scope changes and
  chains dependent tasks; broad work started another way is stopped and offered as a supertask.
  Agents claim the paths they work on (`scope_claim`), and an overlap names the holding task.
- **Waiting and parking.** A queued task says what it waits for ("8 of 8 in use"). A message sent
  to a task that has not started is added to its request. A task whose turn ended and that waits
  for a person is **parked** after 30 idle minutes: its process ends and frees the session slot,
  and it keeps its place, last screen, question and pull request. Writing to it or Resume continues
  the same session. An open step, unfinished spec or unfinished goal keeps it live.
- **Fork…** starts a new task from another task's conversation and committed work, with its own
  branch and a copy of the plan, optionally with a different instruction, model and subscription.
  Claude Code and Codex tasks can be forked; Antigravity tasks cannot.
- **Ownership.** A task is its owner's: Yulius acts on GitHub with their credentials (the agent's
  own Git and `gh` use a narrowed App token), and only the owner types into it, messages it,
  continues, pauses or delivers it. Anyone who may work in the project can stop, archive, restore
  and delete its tasks; stopping or deleting someone else's asks first and is audited. Others watch
  its live transcript and answer what it asks under Needs you with **Reply**, without taking
  control. They can **take control** after a confirmation naming the owner, who can take it back;
  a person can ask to be asked first, and then control is requested and handed over.
- **Needs your action** says why on the task: what is wrong, what to do and where. An agent's
  `needs_you` can name the people who should answer.
- **Handoffs** (off by default; an administrator picks Ask or Automatic): a task hands the part of
  its work that belongs elsewhere to another sandbox its owner can work in. Sandboxes can opt out.
- Deleting a task keeps attachments other tasks still use and holds the rest in a 7-day trash.

### Sessions and terminals

- Each task's agent is a real CLI in a PTY. A restart or update of Yulius is invisible to it: its
  Yulius tools and `yulius wait` hold until Yulius is back, a task that went idle meanwhile is told
  "Yulius is back; continue", and a session whose Yulius tools dropped is reconnected.
- Scrolling a terminal past its top offers the whole session from its first message, also after
  Claude Code compacted it. The Sessions page keeps past sessions; agents read and search them with
  `session_read` and `sessions_search`.
- Yulius types into a session only a person's message sent through their Superagent or own agent,
  pull request news, the wake after an outage and the reconnect. It never types over a draft a
  person may have left in the prompt box; the task says so when a message waits.
- Yulius keeps Claude Code current: a new release reaches new and resumed sessions within about an
  hour and a running task when its turn ends; a release that does not know Yulius's default model is
  never used. It provides pinned Codex and Antigravity CLIs.
- **Harness memory is read-only.** Every contained session reads its harness's memory and
  user-level instructions but cannot change them; lasting knowledge goes into the repository
  through a pull request. Plugin language servers are off in every run.

### The discipline: specs, steps, review, delivery and the merge queue

- **Spec when it matters.** A task that needs a spec cannot edit code with its edit tools until
  the spec has a Plan; a turn-end gate checks the rest. Research, questions and documentation need
  none. A defect a task finds is fixed there, not filed and left.
- **Steps.** The agent keeps a plan of effort-weighted steps that holds all the work it knows of; a
  pull request delivers the finished part while later steps stay open. Goal tasks go on through
  their remaining steps by themselves and ask their owner only when a person is needed.
- **Independent review** runs as Yulius's reviewer agent, whose model Yulius picks per round: Fable
  5.2 for a pull request's first two rounds, Opus 5.5 from the third or when no subscription can
  run Fable. Codex tasks review with GPT-6.2 Astra, Antigravity tasks with Gemini 4 Neon. Each round
  is recorded with its model and verdict; agents fix findings in their own code and get a re-review
  without asking anyone. An approval carries across a follow-up of at most 20 product lines. CI
  tests the head while the reviewer reads, and a review survives a usage limit.
- **Delivery.** `delivery_merge` pushes, opens or updates the pull request, checks it against the
  branch policy in `yulius.toml` (typed branches, conventional titles, `dev` → `main` promotions)
  and arms the merge of the reviewed head. Commits made after a merge open a new pull request of the
  same task. Promotion to `main` is a person's.
- **Merge queue** (every sandbox): a pull request's own CI run is never restarted because `dev`
  moved. Armed, green and up to date, it merges at once; otherwise it joins a merge candidate that
  tests it on top of the current integration branch with the others waiting, halved when red, and
  they merge in order as soon as that run passes. `[merge_queue] apart` keeps pull requests that
  touch the same derived files apart. Every armed pull request says where it stands.
  [Delivery and the merge queue](architecture.md#delivery-and-the-merge-queue)
- **Pull request notices.** Yulius types what happens to a task's open pull request into its
  session with the next step: a red run (failing jobs and tests), a conflict, a dropped or refused
  merge, the merge, why a green head is not merging. A quiet session gets a check-in, and a stalled
  pull request is shown as stalled. Agents wait with `yulius wait` and read `ci_status` and
  `delivery_status`; merging by hand is refused with the reason.
- **After the merge**, a task's delivery links the CI verdicts and deployments of its landed commit,
  and a machine-verification step can close by citing them.
- **Changes log.** Each task notes a user-visible change in the project's changes log, or in a new
  note of its own when `[docs] changes` names a folder, so parallel pull requests never conflict.

### CI

- **Yulius CI** is every project's only CI, run on the server (no GitHub Actions) from the
  `[[ci.jobs]]` in force on the integration branch; a bad `yulius.toml` fails the run with the
  parser's message and never turns CI off. Pull requests opened on GitHub by hand start CI as soon
  as GitHub reports them; pull requests from forks are never tested.
- A job can be `exclusive` (one run at a time per sandbox) and set its own time limit. A job that
  checked nothing exits with `$YULIUS_CI_SKIP_CODE` and reads Skipped, never Passed. A job removes
  the Docker containers and networks it created.
- A job gets an agent secret only when the integration branch's entry names it and the job runs
  that entry's command, as a placeholder through the egress proxy.
- A test that fails in the same job on two pull requests within 48 hours is quarantined with a
  flaky-test issue. A red integration branch gets one repair owner, and CI and Status name the
  failing test and the merge that introduced it.
- A run has its own page with its stages and jobs; the CI/CD page's Now summary lists what runs.
- **Vidoc security scan** (optional instance key): agents and the built-in `security` job run
  `yulius security-scan`, which reviews the branch's changes. Open critical or high findings fail
  it, and medium ones in changed lines; findings in untouched code become GitHub issues. The key
  never reaches agents. A sandbox's admins switch scans off for their project, and a superadmin for
  the whole instance, without removing the key; a scan then reports "disabled".

### CD, staging and production

- **CD** is the CI/CD page's second tab. Every project defines dev and production in `yulius.toml`
  (`[cd.dev]`, `[cd.prod]`) and may add more targets (`[cd.<name>]`) that follow a stage's rules,
  each with deploy and verify scripts under `scripts/cd/`. Yulius runs the exact commits of the
  protected branches, dev inside the sandbox and production outside it, each with the secrets and
  project variables allowed for it. Agents improve CD through pull requests but cannot change what
  a deploy runs.
- Dev deploys are open to writers and agents (`cd_deploy`); the ready-made "Deploy dev every night"
  automation deploys and tests dev. Each task tests locally first with the project's `[local]`
  deployment in its own sandbox.
- **Merge-first staging** (a staging target with `merge_first = true`): pull requests merge on the
  quick checks, the full suite runs once on the merged revision, and staging deploys and verifies
  tested revisions in order. A task asks for staging once its delivery merged; staging also moves
  on by itself after 5 merges or 60 minutes (`serve_after_merges`, `serve_every_minutes`). A red
  head waits for the next green one, and the CD page says so.
  [CD, staging and production](architecture.md#cd-staging-and-production)
- **Rollback** is part of every environment that deploys: a command that redeploys a kept release,
  never building. The CD page offers "Roll back to previous" or "Roll back to this" on the three
  newest releases, refuses rolling back past a migration that cannot be undone, and drills the
  rollback on dev after every deploy. An environment that deploys nothing needs none.
- **Production is a person's**: an administrator types the project's slug, and it deploys the
  revision staging verified (also while a newer one is being checked), only once its rollback drill
  passed; when only the drill is missing and dev is not the merge-first staging target, Deploy
  production deploys dev at that commit first, waits for its drill and then deploys production
  by itself, or says why production did not deploy. A
  GitHub ruleset keeps the production branch out of the agents' token (onboarding adds it, or a
  sandbox admin waives it where the plan has none). Where a delivery profile promotes by itself
  (`auto_promote`), Yulius promotes verified changes; the production deploy stays a person's.
- Yulius's own dev deploys to its staging VM; its production is the promotion of `dev` to `main`,
  which each instance installs when a superadmin chooses Update now.

### Secrets, variables and the egress proxy

- **Team secrets** are for people: a title, a value and notes, in one sandbox or team-wide. Reading
  a value needs the person's own two-factor code; values stay masked until shown and copy without
  being shown, multi-line ones with their line breaks.
- **Agent secrets** are used by agents that never see them. An agent gets a placeholder, and the
  egress proxy puts the value only into requests to the secret's bound hosts, audits each use and
  redacts responses; a value that reaches a terminal, transcript or tool result shows as
  `[secret:NAME]`. With "Let agents set it up" (the default), agents bind the hosts they need;
  without it, a person allows each host on the Secrets page. A manager can limit a secret to
  methods and paths (`GET /repos/*`), so a read token cannot write.
  [Agent secret hosts](architecture.md#agent-secret-hosts)
- **Scopes**: Project, Global (defined by administrators, switched on per project) and Personal (one
  person's, used only by tasks they start). A name resolves to the project's secret, then the
  person's, then the global one.
- **`.env` files.** When code needs a real value, the agent asks Yulius to copy it into the
  worktree's git-ignored `.env`, which a person allows once per secret. Agents add a secret from a
  file they can reach without anyone seeing it. They never use a production secret; they ask for a
  copy, and a manager allows it with their two-factor code.
- Widening a secret allowed use by use (a new host, `.env` copies, production) needs that code, and
  an API token cannot do it.
- **Project variables** are plain, non-secret values (regions, ids, base URLs), in every task's
  environment and in CD runs.
- **Agent states**: a deploy tool's live state from a laptop is sent encrypted
  (`yulius secrets import --state`), restored into the sandbox by a task, saved back after each
  rollout, with one writer at a time; CD runs use it too.
- **Brokered commands** run a CLI that needs a key file (a deploy tool, a cloud SDK) outside the
  agents' sandbox. Administrators define it on the Secrets page's Commands tab (binary, secret,
  allowed subcommands, who may run it, which runs need approval); agents run it with `yulius run` or
  `command_run` and get its output with secrets blanked. Approvals happen on the page or by a
  Telegram reply; no answer in time is a no.

### The Superagents

- **Superagent**: each person's own Claude Code session per sandbox, in the panel beside any page,
  for quick work (questions, automations, steering tasks); code belongs in a task. `/clear` starts
  over, and it keeps its conversation across restarts. It has the project's MCP servers, the
  vault's placeholders and the sandbox browser, as a task does.
- It does what its person may do on the project's pages, with their rights and audited as theirs:
  create, continue, pause, reorder, rename, stop, archive and delete tasks; edit a waiting task's
  request; message tasks; answer or dismiss their questions; read deliveries, CI runs and failing
  output, rerun or cancel runs; merge a task's pull request; deploy dev; move a task to another
  subscription or change its model; read the instance and, for a superadmin, update or restart it.
  It reads a task's screen, its token use and how far its checkout is behind.
- Some steps wait for the person on its panel: work in another of the person's sandboxes (the whole
  request shown, nothing created until they press Create task), production deploys, rollbacks and
  promotion (typed slug), starting onboarding, a secret's value (typed by the person, masked, never
  seen by the Superagent), and decisions answered Yes or No. Such requests last until answered.
- It does not answer a permission menu in a task's terminal (it tells its person) and never sees a
  secret's value. A missing ability it reports with `ability_report`, which opens a ready issue.
- A standing job ("check the disk every hour") is a scheduled check Yulius holds and types into the
  Superagent when due, starting it if needed; a check a script can decide is a script watch.
- **Global Superagent** (the panel's All sandboxes tab, or `/superagent`): one per person across
  every sandbox they can see, turning "tell every running task to merge dev" into tasks, pull
  requests and messages in each, as the person. It works only through Yulius's tools; deploying,
  secrets, variables and adding projects stay the person's.
- **Task agents** can create, chain, schedule, stop, archive and delete tasks and automations for
  their owner (not their own task) and read where other tasks stand (`tasks_list`, `task_get`), so
  they look before overlapping; they do not message or steer other tasks.

### MCP servers

Configured once on the MCP page, for every sandbox or one, and for the whole team or only their
owner; a personal server reaches only its owner's tasks and Superagent. Their secrets may come from
agent secrets. A repository's `.mcp.json` servers join automatically, with `${VAR}` becoming the
agent secret's placeholder; Test does not run such a server's command until a person changes it.

### Notifications, Telegram and the digest

- **Notices** have a key and an expiry: a newer notice about the same thing replaces the older one
  (in the bell, Telegram, the digest and the Status and CI pages), so a task has one entry. Reports
  expire after a day, good news after three, failures after seven; a notice waiting for a decision
  stays until it is answered.
- **Telegram notifications**: an administrator sets one bot for the instance; each person links
  their own chat with a one-time code and picks what reaches it (Needs you by default). Messages
  are redacted and rate-limited, and no inbound port is needed.
- **Work from Telegram** (off by default, per chat): a reply to a notification answers its task; a
  new message, with photos, documents or voice notes, starts a task as the person in the chosen
  project; `/task <project> <text>` picks another, `/status` lists what waits.
- **End-of-day digest** (off by default): once a day, what waits for each person, what was parked
  and how many tasks still work, in the bell and Telegram; it can first park quiet tasks.

### Automations and script watches

- **Automations** are tasks on a schedule, started any way New task starts them, running as their
  creator; only the creator edits them, switches their schedule or uses Run now, and project writers
  can delete them. Three ship switched off: "Keep the docs up to date", "Work through open issues"
  (the five most important actionable issues, a task each) and "Deploy dev every night".
- **Script watches** run a bash script on a schedule in the project's sandbox without any agent or
  subscription, so watching a URL, a release, a CI number or a date costs no tokens. Its output
  becomes a keyed notice for its owner; exit 3 posts and ends the watch, and the third failure in a
  row posts one warning. Runs are bounded and get only the agent secrets they name. The Watches page
  and the `watch_*` tools manage them.

### Notes, the editor and the browser

- **Notes**: a notepad per sandbox, one for the team and a private **Me** notepad, saved as people
  type, with teammates' edits and carets live.
- **Editor**: Monaco over the sandbox's files for writers, shared live (teammates see each other's
  keystrokes before saving), saving by itself, following what agents write to disk, and offering
  Keep mine or Take disk version over unsaved text. It opens the sandbox repository or any task's
  worktree; another person's opens read-only.
- **Browser panel**: the sandbox's real Chrome streamed as video, so people pass captchas and
  sign-ins for the agent, which hands them a tab with a link. Pastes arrive as real pastes, and
  text a page copies can be copied to the person's own clipboard. Agents and Superagents drive it
  with `browser_*` tools and sign in with agent secrets they never see, only on their hosts.
- The web UI also holds terminals, the Git view, the Sessions, Workflows and Skills pages, and the
  Dashboard (Overview, Pipeline and Harnesses tabs). Yulius's own skills are packaged primitives
  deployed to every harness; onboarding codifies a project's conventions the same way.

### Subscriptions and usage limits

- Claude, Codex and Antigravity accounts form one self-managing pool per harness; a task moves
  only between subscriptions of its harness, and agents do not notice switches. Claude connects by
  its own sign-in in a terminal on the page or a pasted `claude setup-token`; Codex by device code,
  an imported `auth.json` or an OpenAI API key; Antigravity by its Google sign-in or a Gemini API
  key. Until a Claude account is usable, a fresh instance shows Connect Claude everywhere.
- Each task shows its subscription, and "Run #N on…" moves it. A subscription past 95 % of a
  5-hour, weekly or model window gets no new task while another has room, and its idle tasks move
  to the one with the most room in the same session; a task mid-turn moves at its turn's end.
- When a limit cuts a turn, the session moves to a subscription with capacity and goes on, or waits
  for the reset; nobody types "continue". A refused subscription leaves the pool and says why.
- A model's own allowance (Fable's weekly credits) binds only that model; a Fable task goes on with
  Opus 5.5 while no subscription can run Fable. Opus 5.5 has no fallback.
- Cards show the usage reading's age, which window is used up and when it resets, burn rate, tokens
  and the estimated cost at API prices over 7 days, the account's email, a badge for its harness and
  the next charge date once an administrator sets it. Refresh asks the provider again, so bought
  credits end a limit at once.

### Pipeline and the Dashboard

Every stage of tasks, CI runs and deployments is measured and kept 180 days, with what CI jobs and
deploys used (CPU time, peak memory, bytes written). The Dashboard's Pipeline tab shows deployment
frequency, lead time, change failure rate and time to restore, stage percentiles with CSV/JSON
export, review statistics per model, and a waterfall per task, CI run and deploy. The Harnesses tab
compares tasks, run times and tokens per harness.

### Instance operations

- **First run**: a numbered wizard (superadmin, recovery code, the optional Jev and Vidoc keys, the
  first Claude subscription), then getting-started steps on Home: connect Claude, the first
  sandbox, the GitHub App. [First run](operations.md#5-first-run)
- **Host setup.** `sudo yulius host setup` installs what the host needs (Docker for sandboxes, the
  browser policy, the systemd units that keep sessions alive across restarts); every update checks
  it again, and `yulius doctor` proves it.
- **Updates** come from `main`: the superadmin presses Update; once the new version is built, every
  task is frozen mid-turn and thawed after the restart, running deploys finish first, and a build
  that fails to start is rolled back to the previous binary and database. Restart… restarts without
  installing. [Updates](operations.md#updates-from-main-the-update-helper)
- **Resource limits** (sessions, CI jobs and runs, sandbox memory and disk) are sized from the
  host; Settings → Instance shows each value and its source, and a superadmin can pin one.
  [Resource limits](operations.md#resource-limits)
- **Moving the instance**: `sudo yulius instance migrate root@new-host` (or Settings → Instance →
  Move to another host) sets up the new host, copies while the old one runs, freezes only for the
  last changes, proves the copy, starts the new instance and redirects the old URL; any failure
  before then thaws the old one. `yulius instance export | … import` copies the whole instance.
  Settings → Instance's Address card says whether GitHub accepts the instance's callback URL.
  [Move the instance](operations.md#move-the-instance-to-another-host)
- **GitHub outages and budget.** While GitHub is unavailable, finished tasks keep their last pull
  request state and the instance says when merges and CI can use GitHub again. The Status panel
  shows the GitHub App's remaining requests; near the reserve, display yields to delivery.

### The CLI and remote control

- A person makes an API token in Settings → Connect your local agent (operate or read only,
  revocable) and adds Yulius to their own Claude Code with `claude mcp add`, or signs the `yulius`
  CLI in with `yulius remote login`. From the laptop they import and onboard repositories, set up
  CI, create, message, restart, pause, stop, archive and delete tasks, follow transcripts, pull
  requests and CI, deploy dev, and manage automations, watches, variables and secret bindings,
  within their own access. Production deploys and secret values stay in the web UI.
  [Remote control](remote-control.md)
- "Onboard this into Yulius" to the local agent is enough: it uploads the checkout and sessions,
  imports the project's keys by file without passing values through the agent, and starts
  onboarding with a hand-over. `yulius secrets import` lists every name, overwrites nothing and
  sends nothing before `yes`; plain values go to project variables (`yulius vars`).
- Inside a session, agents use `yulius wait`, `yulius run`, `yulius tool`, `yulius remove` and
  `yulius security-scan`.

## Principles

Agents are real CLIs; sessions outlive the server; GitHub is the authority for permissions (a
playground's guests, let in through an expiring link, are the one exception), and the person who
creates an organization's project in Yulius is made its repository's admin there; coordination goes
through one MCP and hook protocol; every queue, stream, retry and wait is bounded and owned;
features have specs, research and questions do not; a defect a request reports is fixed in its
task, also when it is asked as a question, never filed and left.

## Where to read more

- How it is built: [architecture.md](architecture.md); why: [decisions](decisions/README.md).
- Running an instance: [operations.md](operations.md); driving it from a laptop:
  [remote-control.md](remote-control.md); the HTTP and MCP API: [api.md](api.md).
- How work is done here: [engineering/workflow.md](engineering/workflow.md),
  [engineering/specifications.md](engineering/specifications.md),
  [engineering/git.md](engineering/git.md).
- What changed recently: [changes.md](changes.md). The specs in `.ai/specs` are history: why
  something was built, not what is current.
- Every feature in detail, with its spec and decision citations:
  [archive/product-until-2026-11-26.md](archive/product-until-2026-11-26.md).
