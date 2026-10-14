# Architecture

One Rust binary, `yulius`, serves the web UI and API, supervises detached `ptyhost` processes
that own Claude Code sessions inside per-sandbox containment, and talks to GitHub and
Anthropic. The detailed contracts are in
[SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md); crate layering rules are in
[engineering/architecture.md](engineering/architecture.md).

```mermaid
flowchart LR
  UI["Web UI (xterm.js, Monaco)"] <--> RP["Reverse proxy (TLS)"] <--> API
  subgraph Server["yulius serve"]
    API["Public HTTP + WS API"]
    INT["Internal loopback: MCP, hooks, git and gh credentials"]
    APP["Application services"]
    SUP["Session supervisor"]
    CI["Yulius CI runner"]
    DB[("SQLite WAL")]
  end
  API --- APP --- DB
  APP --- SUP
  APP --- CI
  APP -->|"prune caches, quota"| SB
  SUP <-->|"Unix socket"| PH["yulius ptyhost (per run)"]
  subgraph SB["Sandbox (namespace holder)"]
    PH --> AG["claude TUI"]
    CH["Chrome on Xvfb"]
  end
  AG -->|"MCP + hooks"| INT
  APP <--> GH["GitHub App + REST"]
  AG <--> AN["Anthropic"]
  APP --- CH
```

## Layers and crates

Dependencies point inward: interface, composition and adapters depend on application, which
depends on domain and contracts. `scripts/check_repository.py` enforces this.

| Layer | Crates |
| --- | --- |
| domain | `yulius-domain` |
| contracts | `yulius-contracts` |
| application | `yulius-app` |
| adapter | `yulius-store`, `yulius-ptyhost`, `yulius-sandbox`, `yulius-harness`, `yulius-github`, `yulius-git`, `yulius-browser`, `yulius-egress`, `yulius-import` |
| interface | `yulius-server` (axum routes, embedded web UI in `web/`) |
| composition | `yulius` (configuration, wiring, subcommands) |
| tooling | `tools/` (fake agent, xtask) |

## CPU

A sandbox's processes share its cgroup limits, but not equally. CI jobs run in its `ci/` leaf
with four times the CPU weight of the agents' `agents/` leaf. Under full load CI gets up to 80 % of
the sandbox and the agents at least 20 %, and idle CPU goes to whoever wants it. Agent Bash commands also run
5 nice levels below the agents themselves ([SPEC-053](../.ai/specs/SPEC-053-ci-e2e-timeouts-under-agent-load-give-ci.md),
[decision 0039](decisions/0039-ci-jobs-get-the-cpu-first.md)).

```mermaid
flowchart TD
  SBX["sandboxes/sandbox: cpu.max, memory.max, pids.max"] --> SYS["system/ weight 100: holder, slirp4netns"]
  SBX --> AGS["agents/ weight 100: agent runs, shells, browsers"]
  SBX --> CIG["ci/ weight 400: CI jobs"]
```

## Disk

A sandbox has a soft disk quota. The application prunes build caches of tasks that do not
work every 5 minutes (ended tasks, then idle ones from 90 % of the quota), removes the worktrees
of closed and cancelled tasks, and a switch checks the quota and the other start preconditions
before it ends the running session ([SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md),
[decision 0017](decisions/0017-sandbox-disk-upkeep.md)). A running task's caches go only when a
fresh probe finds no background shell and no background subagent. An unanswered probe keeps them.

Over the quota, a CI run waits in the queue instead of erroring
([SPEC-059](../.ai/specs/SPEC-059-fix-240-256-258-245-ci-waits-for-disk-in.md),
[decision 0045](decisions/0045-ci-waits-for-disk-under-the-shared-quota.md)). The dispatcher
starts a check off its loop, at most once a minute per sandbox: upkeep under pressure, then a fresh
measurement. The run's reason and its `pending` commit status name the largest consumers, and the
run starts at the first dispatch under the quota. A job refused after its run started puts the
run back in the queue.

```mermaid
flowchart LR
  Q[queued run] -->|dispatch| R{under the quota?}
  R -->|yes| L[lane: jobs run]
  R -->|no, check due| C[prune caches of tasks that do not work, measure]
  C -->|still over| N[reason + largest consumers, status pending, recheck in 1 min]
  C -->|under| L
  N --> Q
  L -->|job refused for disk| Q
```

## Usage limits

When a subscription's usage limit cuts a turn, the task goes on as if the limit never happened
([SPEC-034](../.ai/specs/SPEC-034-in-previous-tasks-we-have-hit-a-session.md),
[decision 0027](decisions/0027-step-back-past-a-usage-limit.md)): on another subscription at
once, or on its own after the reset. The supervisor has the harness adapter step the transcript
back over the limit on a blocking thread, and indexes the session again only if the file changed.
Claude Code then resumes the same session with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1`, so it
redoes the lost turn by itself. Nothing is typed. If Claude Code continues the turn by itself at
the reset, its first agent work or turn end after the pause ends the pause, and Yulius leaves
that run alone; after a restart, a limit that such a turn followed is not acted on again. A person's
switch of a task left at its limit by a refused automatic switch also resumes past the limit
([SPEC-072](../.ai/specs/SPEC-072-fix-276-277-278-usage-limit-resume-respe.md),
[decision 0058](decisions/0058-the-clis-own-continuation-ends-a-limit-pause.md)).

```mermaid
sequenceDiagram
    participant C as Claude Code (account A)
    participant T as Transcript (JSONL)
    participant S as Supervisor
    participant H as Harness adapter
    participant N as Claude Code (account B)
    C->>T: usage-limit error + limit notices
    T->>S: LimitReached (transcript watcher)
    S->>C: SIGTERM (idle at the limit)
    S->>H: plan Resume { continue_interrupted }
    S->>H: step_back_transcript (spawn_blocking)
    H->>T: step back: drop errors and notices, relink parentUuid
    H->>N: claude --resume id (flag set, no prompt)
    N->>T: the lost turn, redone
```

## Full context

A session whose context is full gets "Prompt is too long" for every prompt, and no Stop hook ends
the turn ([SPEC-060](../.ai/specs/SPEC-060-fix-213-a-session-whose-context-is-full.md),
[decision 0046](decisions/0046-compact-then-restart-a-full-session.md), #213). The harness
observes the refusal (`ContextFull`), the compaction boundary (`Compacted`) and a failed compaction
(`CompactFailed`). The supervisor ends the turn so the task waits instead of working, and keeps
one recovery episode per task in its runtime. The episode outlives a run and ends at the agent's
next successful reply. Each stage is said in the task's terminal and audited as
`task.context_full`. A replay after a restart that ends in a refusal starts the same recovery.

```mermaid
stateDiagram-v2
    [*] --> Clear
    Clear --> Compacting: ContextFull / type "/compact <focus>"
    Compacting --> Compacted: Compacted / type the wake-up line
    Compacting --> Restarted: CompactFailed or 10 min / fresh session + handoff
    Compacted --> Restarted: ContextFull / fresh session + handoff
    Restarted --> GaveUp: ContextFull in the fresh session / needs attention + reason
    Compacting --> Clear: a successful reply
    Compacted --> Clear: a successful reply
    Restarted --> Clear: a successful reply
    GaveUp --> Clear: a successful reply
```

## Playgrounds

A playground lets a team try Yulius on its own code before it connects GitHub
([SPEC-099](../.ai/specs/SPEC-099-on-onboarding-we-want-a-mode-to-play-aro.md),
[decision 0081](decisions/0081-a-playground-keeps-its-git-on-the-server.md)). Its `origin` is a
bare repository on the server outside the sandbox (`<data>/playgrounds/<id>.git`), so the
clone, worktrees and merges use the same `origin/<branch>` refs as a GitHub sandbox while nothing
leaves the server, and the sandbox's processes never reach a repository whose hooks the server's
Git would run; the server fetches and pushes it by that path, never by the clone's configured URL.
The record marks it with an empty repository owner. The browser packs a chosen folder into a gzip tar (`files/` the
working tree, `git/` the history part of `.git`); the server takes only `HEAD`, `packed-refs`,
`shallow`, `objects/` and `refs/` of it, copies the branches and tags into the new `origin` with the
hardened Git, and commits the working files on top. `delivery_merge` squashes a task branch onto
the integration branch there (`git merge-tree` and `commit-tree`, pushed without forcing). CI
sync, hygiene, the automatic pull request and reconciliation skip playgrounds.

```mermaid
flowchart LR
  B["Browser: folder → tar.gz"] -->|"POST /api/playgrounds"| S["Server: unpack files/ and the history of git/"]
  S -->|"init --bare, copy refs, commit the files"| O[("data/playgrounds/id.git (outside the sandbox)")]
  O -->|"clone; fetch by path"| R["sandbox/repo"]
  R -->|"worktree from origin/dev"| T["Task worktree (agent, contained)"]
  T -->|"delivery_merge: Yulius squashes, pushes by path"| O
```

## Parallel tasks at merge

Tasks working at once must not collide where their parallel work meets
([SPEC-083](../.ai/specs/SPEC-083-merge-conflicts-with-many-agents.md),
[decision 0066](decisions/0066-parallel-tasks-stop-colliding-at-merge.md)). Numbered files
(decision records, specs, migrations) get their number from Yulius (`record_number`): past the
integration branch, every live task's checkout and every reservation in `record_numbers`. The logs
every task appends to (the changes log, the decision index) merge by union in every repository,
from Yulius's block in the clone's `info/attributes`; Yulius merges the integration branch
locally before GitHub merges (SPEC-050 DLV-203), so GitHub never sees those conflicts. A review
carries over Git's own merges of the integration branch, checked with `git merge-tree`.

## Agent feedback

Yulius has no messages for now ([SPEC-028](../.ai/specs/SPEC-028-delete-message-mechanism-for-now-until-w.md),
[decision 0023](decisions/0023-no-messages-until-delivery-is-reliable.md)): nothing is stored for
an agent, injected by a hook or typed into its session. An agent pulls what it needs: it waits
with `yulius wait` in a background shell, which Claude Code wakes it from, and reads
`ci_status` and `delivery_status`. The armed merge's outcome is kept in memory for the wait,
with the armed merge and its decisions in progress under one lock: a merge armed again while an
earlier one is decided supersedes it ([decision 0048](decisions/0048-armed-merge-state-under-one-lock.md)).
Before Yulius merges or arms a merge, its merge gate checks the pull request's title, head and
base against the branch policy of `yulius.toml` `[branch_policy]` at the integration branch;
a violation is refused with the rule in words, also for promotion pull requests
([SPEC-069](../.ai/specs/SPEC-069-fix-223-yulius-s-merge-gate-enforces-the.md),
[decision 0051](decisions/0051-the-merge-gate-enforces-the-branch-policy.md)).
`delivery_merge` first delivers the head the way the end of a clean turn does
(`try_auto_open_pr`): it pushes the branch and opens the pull request, or pushes new commits to
it. A head it cannot deliver yet (open step, no plan for a task that needs a spec, invalid spec,
uncommitted work) is refused with the reason, and a push GitHub refuses answers with the remedy
(merge the pushed head, never force). A goal task whose evaluator keeps every turn going therefore still gets its pull request
([SPEC-116](../.ai/specs/SPEC-116-linter-dead-code-should-be-installed-on.md) DMG-001).

```mermaid
sequenceDiagram
    participant A as Agent (Claude Code)
    participant W as yulius wait (background shell)
    participant Y as Yulius (internal API)
    A->>Y: delivery_merge (head pushed and pull request opened if missing; policy checked; checks running: armed)
    A->>W: start, end the turn
    W->>Y: POST /internal/agent/wait (long poll)
    Y-->>W: checks ended / stage moved / armed merge decided (merged, refused, dropped)
    W-->>A: one line, then exits: Claude Code wakes the agent
```

## Task start

A task's agent is a native Claude Code session. Files attached on the new-task composer are
uploaded to the sandbox's drafts folder and reach the session's first message natively. Without
an image, that message is the CLI prompt argument and each file is mentioned there (`@path`).
With an image, the session starts with no prompt and Yulius types the message into the TUI. Each
image's path is pasted on its own, and Claude Code shows it as `[Image #N]` and sends an image
block. The text is pasted next, with the other files mentioned, then Enter (an `@` mention of an
image would only be a Read tool call). A goal task then gets its `/goal` typed into the terminal;
the request file in the sandbox home keeps the whole request. A person who typed into the
terminal before the goal keeps their draft: no goal is typed and the task needs attention (only
keystrokes count as typing; focus and mouse reports and terminal answers do not). Each
launch trusts the worktree and its main repository in the Claude config. Claude Code 2.1.292 looks
a worktree's trust up there when `.claude/settings.json` pre-approves tools. If a trust prompt
still shows before SessionStart, Yulius types nothing into it, and the keys a person answers it
with are no draft: the goal follows once it is gone, and a prompt left unanswered for 5 minutes
flags the task
([SPEC-106](../.ai/specs/SPEC-106-onboarding-another-repo-got-fucked-up-by.md),
[decision 0086](decisions/0086-a-lost-onboarding-task-returns-the-sandbox-to-pending.md)). A typed
first message is pending (`opening-pending` in the run directory) until its `UserPromptSubmit`: a
restart flags a task whose first message never reached that hook (a submitted one a hold keeps
goes on, and a goal task's goal is typed after it), and a first message held for capacity resumes
on another account, or a manual switch, as a fresh session that types it again with its images
([SPEC-068](../.ai/specs/SPEC-068-fix-252-266-only-real-typing-counts-as-t.md),
[decision 0052](decisions/0052-only-keystrokes-count-as-typing.md);
[SPEC-011](../.ai/specs/SPEC-011-automatic-dev-delivery.md),
[SPEC-025](../.ai/specs/SPEC-025-uploaded-attachements-should-be-clickabl.md),
[SPEC-030](../.ai/specs/SPEC-030-i-asked-previous-task-to-attach-images-a.md),
[decisions 0022](decisions/0022-native-task-attachments.md) and
[0026](decisions/0026-attached-images-are-pasted.md)). The request is read when the task first
starts, after its compare-and-set to `starting`. Until then, a message a person queues into the
waiting task is appended to the stored request by one conditional update, so the start either
reads it or refuses it ([SPEC-052](../.ai/specs/SPEC-052-even-if-a-task-is-queued-i-d-like-to-be.md),
[decision 0035](decisions/0035-queued-messages-join-the-request.md)).

```mermaid
sequenceDiagram
    participant UI as Composer
    participant S as Yulius server
    participant C as Claude Code (PTY)
    UI->>S: upload each file (drafts folder)
    UI->>S: create task (prompt + @path lines)
    alt no attached image
        S->>C: launch with first message: text (prompt mode) or "files attached" (goal mode) + @path mentions
        C-->>C: attaches each mentioned file
    else attached images
        S->>C: launch with no prompt
        C->>S: SessionStart hook, then quiet output
        loop each image
            S->>C: bracketed paste of its path
            C-->>S: screen shows [Image #N]
        end
        S->>C: bracketed paste of the text (+ @path mentions of other files), Enter
        C->>S: UserPromptSubmit: the first message is no longer pending
    end
    C->>S: Stop hook of that turn (goal mode)
    opt Claude Code shows its folder-trust prompt
        S-->>UI: notice: answer it in the terminal
        UI->>C: a person answers (not a draft)
    end
    alt a person typed since the launch
        S-->>UI: no goal; task needs attention, notice in the terminal
    else
        S->>C: types /goal <condition>
    end
```

### Onboarding repair

A sandbox waits in `pending` or `preparing` until its onboarding task has made it ready
([decision 0076](decisions/0076-onboarding-is-mandatory-in-the-ui.md),
[decision 0077](decisions/0077-onboarding-task-is-created-when-needed.md)). That task stays in
the task list while the sandbox waits: archiving it is refused. If it is deleted, the sandbox
returns to `pending` and Start onboarding creates a new one. `refresh_onboarding` makes these
repairs, and the server makes them once for every sandbox at start, which heals sandboxes stuck
before the fix ([SPEC-106](../.ai/specs/SPEC-106-onboarding-another-repo-got-fucked-up-by.md),
[decision 0086](decisions/0086-a-lost-onboarding-task-returns-the-sandbox-to-pending.md)).

```mermaid
stateDiagram-v2
    [*] --> pending: import
    pending --> preparing: Start onboarding (creates the task)
    preparing --> ready: task finished, format check passes
    preparing --> pending: onboarding task deleted (repair, also at server start)
    pending --> skipped: admin skip
    preparing --> skipped: admin skip
```

The onboarding task leaves the project ready for the discipline. In its own pull request it lays
the gate: a test command, lint and format, and a dead-code and unused-dependency check, wired into
`[commands]` and `[[ci.jobs]]` with a CI job for every project of a monorepo. Existing findings
go into a committed baseline instead of application-code changes. It delivers through the review
and `delivery_merge` like every task
([SPEC-116](../.ai/specs/SPEC-116-linter-dead-code-should-be-installed-on.md),
[decision 0094](decisions/0094-onboarding-installs-the-gate-in-its-own-pull-request.md)). The
briefing quotes the newest end of the changes log: a log whose ISO dates run oldest first is
quoted from its end, and its new entries go there.

### Supertasks

Every run starts through `App::start_run`: a new task, the queue, a released chain or rule,
Start now, Continue, and the capacity loop. Before the session limit, it applies the supertask
hold (`yulius-app/src/supertask.rs`,
[SPEC-071](../.ai/specs/SPEC-071-somewhere-here-the-user-should-be-able-t.md),
[decision 0054](decisions/0054-a-supertask-holds-its-project-at-the-start-door.md)). A task
without a live run waits `queued` while its project has a pending supertask (only an earlier one
counts, for a supertask). A supertask also waits while the project has a working task. Live runs
are never touched. The queue offers free slots to supertasks first, then FIFO, one start after
the other. A held task's summary lists what it waits for (`held_by`).

```mermaid
flowchart LR
    N[new / queued / released / continued / resumed task] --> G{start_run: held by a supertask?}
    G -->|plain task, project has a pending supertask| Q[stays queued, held_by = supertasks]
    G -->|supertask, project has working tasks or an earlier supertask| Q2[stays queued, held_by = those tasks]
    G -->|not held| L{session slot free?}
    L -->|no| Q3[queued]
    L -->|yes| R[launch]
    E[a run ends or is Done] --> D[drain_queue: supertasks first, then FIFO]
    D --> G
```

### Prepared tasks

A task can wait as `prepared` before its first run (`yulius-app/src/prepared.rs`,
[SPEC-105](../.ai/specs/SPEC-105-add-scheduling-when-i-decide-in-this-mod.md),
[decision 0085](decisions/0085-a-prepared-task-waits-for-a-person-or-its-time.md)). Without a time
it waits for a person's Start now; with one (a row in `task_schedules`) the background tick
releases it once the time has come and the sandbox's onboarding no longer blocks tasks. Released,
it goes `prepared → queued` by compare-and-set and starts through `start_run` like a new task.

```mermaid
flowchart LR
    C[New task: When I decide / At a time] --> P[prepared]
    P -->|Start now| Q[queued]
    P -->|tick: time due, onboarding done| Q
    P -->|PUT schedule| P
    P -->|Cancel| S[stopped cancelled]
    Q --> R[start_run: supertask hold, session limit, capacity]
```

## Superagent (the Cockpit)

Every user has one plain Claude Code session per sandbox, shown as the **Superagent**
([SPEC-074](../.ai/specs/SPEC-074-i-just-hit-usage-limit-in-the-agent-it-s.md), formerly the Agent)
([SPEC-021](../.ai/specs/SPEC-021-in-this-bar-we-need-to-add-cockpit-at-th.md),
[decision 0020](decisions/0020-cockpit-as-a-plain-shell-session.md)); the code and the API still
call it the Cockpit. It is a sandbox shell of kind `cockpit` that runs `claude` instead of the
login shell: no hooks, settings or briefing, a pool subscription's token, and the newest
conversation resumed whenever it starts again. Its start, like a terminal's, runs detached from
the request, so a client that goes away mid-start never leaves a second or half-started session
([SPEC-077](../.ai/specs/SPEC-077-fix-250-cockpit-and-shell-starts-survive.md),
[decision 0053](decisions/0053-shell-starts-run-detached-from-the-request.md)). The web UI shows it in the right tool panel beside
the page, under a notice that it is for quick work only
([SPEC-035](../.ai/specs/SPEC-035-move-cockpit-to-the-right-and-change-coc.md),
[decision 0036](decisions/0036-agent-in-the-right-panel.md)).

A Cockpit remembers its pool subscription (in `shell.json`) and watches its newest conversation
while it runs ([SPEC-074](../.ai/specs/SPEC-074-i-just-hit-usage-limit-in-the-agent-it-s.md),
[decision 0055](decisions/0055-cockpit-moves-past-a-usage-limit.md)). A usage limit recorded after
it started is recorded on that subscription as a task's is. The Cockpit then starts again, under
its start lock, on a subscription with capacity. It resumes the conversation stepped back over the
limit with Claude Code's interrupted-turn flag, as a task does after a limit (SPEC-034). The old
process ends with the exit status `switched`, and the panel follows it to the new one. Without
capacity, it stays and its viewers get a notice.

It manages the project through the Yulius MCP server
([SPEC-037](../.ai/specs/SPEC-037-i-got-something-like-this-tasks-and-cock.md),
[decision 0029](decisions/0029-cockpit-manages-through-mcp.md)). Its bearer (`ck_…`) is derived
from the shell's persisted host token, and it is served only the management tools: tasks,
automations, workflows and sessions. Those tools act for a person (`ProjectCaller`): the Cockpit's
owner, or a task's owner when a task's agent calls them. `task_create` takes the same path as
New task; `task_stop`, `task_archive`, `task_delete`, `automation_delete`, `automation_run_now`
and `automation_template_enable` call the UI's own functions with that person and the caller as
the audit actor, never on the caller's own task
([SPEC-066](../.ai/specs/SPEC-066-fix-275-293-cockpit-and-agents-manage-ta.md),
[decision 0049](decisions/0049-management-tools-cover-the-task-and-automation-lifecycle.md)).

A repository's `.mcp.json` would make Claude Code ask about each of its servers when the
Superagent starts. Before every task run and Superagent start, `App::import_project_mcp` adds
the file's servers that the sandbox lacks to its MCP servers, so task runs get them like any
other server. The Superagent starts with `--settings` naming every one of them in
`disabledMcpjsonServers`, so it never asks
([SPEC-095](../.ai/specs/SPEC-095-while-onboarding-other-project-i-got-som.md),
[decision 0078](decisions/0078-repository-mcp-json-feeds-the-mcp-section.md)). An MCP server's
secret values reach a run in `YULIUS_MCP_<SERVER>_<VAR>` variables, never under the server's own
variable name, which may hold an agent secret's placeholder.

```mermaid
flowchart LR
  UI["Right panel: Superagent"] -->|"POST /api/sandboxes/sid/cockpit"| APP["App::cockpit_open"]
  APP -->|"pool_account"| POOL[("Subscriptions")]
  W["Cockpit watch: newest conversation"] -->|"usage limit after the start: record_limit, start again (switched)"| APP
  APP -->|"plan_cockpit"| HP["Harness: claude [--resume id] --mcp-config yulius [--settings: .mcp.json servers rejected]"]
  APP -->|"import_project_mcp (also before task runs)"| MS[("Sandbox MCP servers")]
  REPO[".mcp.json in the repository"] --> APP
  APP --> SH["Shell runtime (user space, repository)"]
  UI -->|"WS /api/shells/id/terminal"| SH
  SH -->|"Bearer ck_… (YULIUS_COCKPIT_TOKEN)"| MCP["/mcp: management tools"]
  AG["Task agent"] -->|"Bearer task token"| MCP
  MCP -->|"task_create"| NT["App::create_task_as (as New task)"]
  MCP -->|"task_stop / task_archive / task_delete"| TU["The UI's stop, archive, delete (never the caller's own task)"]
  MCP -->|"automation_*, automation_template_enable"| AU["Automations and ready-made ones"]
```

## Agent secret hosts

An agent secret is used only on the hosts it is bound to (SPEC-002 ADR-20). Since SPEC-121
(ADR-0098) a person adds one with a name, a value and a description, and it starts bound to
nothing. The agent works out the service's host and proposes it; the request waits on the secret
(`requested_hosts`) until a person who manages it allows it. An agent never binds a host itself,
so an agent misled by injected content cannot send a secret to another server.

```mermaid
sequenceDiagram
    participant P as Person
    participant UI as Secrets page
    participant App as Yulius (vault)
    participant A as Agent
    participant X as Egress proxy
    P->>UI: Add VIDOC_API_KEY (name, value, description)
    UI->>App: POST agent-secrets (hosts [], injection none)
    A->>App: secrets_list, then secret_request_hosts(api.vidoc.com)
    App-->>A: requested, waiting for a person (task needs attention)
    A->>X: request to api.vidoc.com with the placeholder
    X-->>A: forwarded without the secret (not bound yet)
    P->>UI: Allow
    UI->>App: POST agent-secrets/{id}/request {allow: true}
    A->>X: request to api.vidoc.com with the placeholder
    X-->>A: placeholder replaced by the value, response redacted
```

## Task ownership

A task acts on GitHub with one person's token, so it is that person's: its owner, the creator
until a hand-over ([SPEC-070](../.ai/specs/SPEC-070-tasks-should-be-owned-by-a-specific-pers.md),
[decision 0060](decisions/0060-a-task-is-owned-by-one-person.md)). `tasks.owner_id` decides the
token (`App::owner_github_token`), the worktree's commit identity and the authority of the task's
agent; `created_by` stays who asked. `App::load_task` refuses every task change to anyone but the
owner, and the terminal opens read-only for others. Anyone else who may work in the project asks
for control; the owner hands it over or declines, and an absent owner's task passes at once.

```mermaid
flowchart LR
  B["Bob: Request control"] -->|"POST /control-request"| REQ[("tasks.control_requested_by")]
  REQ -->|"owner absent"| PASS["pass_control"]
  A["Alice (owner): Hand over"] -->|"POST /control-request/accept"| PASS
  A -->|"DELETE: decline"| REQ
  PASS --> OWN[("tasks.owner_id = Bob")]
  OWN --> TOK["Git, gh, PRs, merges as Bob"]
  OWN --> ID["worktree commit identity: Bob"]
  OWN --> GATE["load_task: only Bob writes"]
```

## Ready-made automations

Ready-made automations are a read-only catalog like the default workflows
([SPEC-042](../.ai/specs/SPEC-042-create-2-default-automations-that-can-be.md),
[decision 0034](decisions/0034-ready-made-automations-catalog.md)): `catalog/automations/*.md`,
embedded at build time. Enable copies one into an ordinary automation; a project's automation of
the same name counts as the template's.

```mermaid
flowchart LR
  CAT["catalog/automations/*.md"] -->|"build.rs embeds"| TPL["automations::templates"]
  UI["Automations page: Ready-made"] -->|"GET automation-templates"| TPL
  UI -->|"POST …/enable"| EN["App::enable_automation_template"]
  EN -->|"same name exists"| UPD["re-enable it"]
  EN -->|"else"| INS["insert automation (enabled)"]
  INS --> SCH["Scheduler: one task per slot"]
  UPD --> SCH
```

## How an automation's tasks start

An automation stores one start choice for every task it creates, the choices of New task's start
chip ([SPEC-078](../.ai/specs/SPEC-078-we-d-like-to-be-able-to-set-the-automati.md),
[decision 0062](decisions/0062-an-automation-starts-its-tasks-like-new-task.md)). A run creates
its task through the same `create_task_ruled` as New task. A task to run after that has ended
turns into the rule `#N done`, so a recurring automation keeps working once its predecessor is
finished. The editor shows the choice as New task's start chip in its action bar, and Run now
sends the choice the chip shows for that run only
([SPEC-093](../.ai/specs/SPEC-093-automation-editor-the-start-choice-sits.md),
[decision 0074](decisions/0074-run-now-uses-the-start-choice-the-editor-shows.md)); without it a
run uses the stored choice.

```mermaid
flowchart LR
  RUN["Scheduler or Run now: one slot"] --> START{"automation.start, or Run now's body"}
  START -->|"Now"| Q["queued"]
  START -->|"Supertask"| ST["queued supertask (SPEC-071)"]
  START -->|"After #N, not ended"| CH["chained after #N"]
  START -->|"After #N, ended"| RD["chained on rule #N done"]
  START -->|"Rule"| RL["chained on the rule"]
```

## Web UI state

The web UI is plain ES modules. Server state comes from the API and the UI event stream
(`store.js`); what only one person's browser needs stays in `localStorage`: preferences (theme,
column widths, model and effort), the notification history, the new-task composer's drafts
([decision 0021](decisions/0021-composer-drafts-in-the-browser.md)), and each sandbox's pull
requests as the last Git overview read said them (`yulius.pulls.<user>.<sandbox>`), shown at once
after a reload, for a sandbox a view watches, until the page's own read replaces them (dropped if
that read fails; SPEC-104). The overview is read every minute while the tab is in view, and at once
when the server announces a change of the sandbox's pull requests (`pulls.updated`: a delivery
stage moved, a Yulius CI run was queued, started or ended), at least 3 s after the start of the
last read, or when a hidden tab that missed one is shown again. Reads of a sandbox never overlap,
also across views that stop and start watching it; an announcement during a read brings one more
read after it. A pull request Yulius recorded as merged reads merged even while GitHub still lists
it open ([decision 0087](decisions/0087-pull-request-changes-are-announced.md)).
Sign-out removes the notification history, the drafts and the pull requests; an expired session
removes the pull requests too.

Screens keep their state when the person moves away and back
([decision 0092](decisions/0092-screens-remember-where-they-were-left.md), SPEC-113). The router
(`router.js`) remembers, in `sessionStorage` for this browser tab, the last address shown inside
each sandbox and on each sandbox page, including the filters pages keep in their query with
`replaceQuietly`; links marked `data-restore` (sandbox tabs, Home's cards, the sidebar's pages, the
Tasks and Git back links) go back there, and a link to where the person already is opens the plain
path. The tool panel (`panels.js`) keeps each sandbox's open tool, and the Superagent, Browser,
Editor and Shell views of the 3 sandboxes used last stay alive, hidden; the editor of a sandbox in
the background keeps its files' text without Monaco models. Sign-out and an expired session forget
the places.

Every drop-down menu is built by `openMenu` (`dom.js`). It stays inside the window, scrolling its
items, and a menu of 8 or more items opens with a filter box
([decision 0030](decisions/0030-menus-filter-when-long.md)). A group of items under a header (the
start menu's tasks) shows five rows and scrolls the rest in a box of its own, fading at the edge it
continues past with a "↓ N more" line under it ([decision 0083](decisions/0083-long-menu-groups-scroll-five-rows.md)).

```mermaid
flowchart LR
  C["Composer (taskview.js)"] -->|"edit, attach"| D["drafts.js (per user and sandbox)"]
  C -->|"upload"| U["POST /api/sandboxes/:id/attachments"]
  D -->|"debounced, on leave or hide"| LS[("localStorage yulius.draft.*")]
  LS -->|"after a reload"| D
  C -->|"task started: clear"| D
  SO["Sign-out"] -->|"forget"| LS
  G["GET /api/sandboxes/:id/git"] -->|"every read"| P["store.js loadPulls / pullsOf()"]
  EV["pulls.updated (stage moved, CI run changed)"] -->|"read again"| G
  P -->|"write"| LP[("localStorage yulius.pulls.*")]
  LP -->|"after a reload, until the read answers"| P
  SO -->|"forget"| LP
  EX["Session expired"] -->|"forget"| LP
  R["router.js emit / replaceQuietly"] -->|"each address in a sandbox"| PL[("sessionStorage yulius.places")]
  PL -->|"data-restore links: tabs, Home cards, sidebar pages"| R
  SO -->|"forget"| PL
  EX -->|"forget"| PL
```

## Task status

A running task's activity is derived from Claude Code's own records, not guessed from the last
event ([ADR-0044](decisions/0044-task-status-follows-claude-codes-own-records.md)). Background work
is counted from three sources:
- background shells, from the process probe;
- background agents, from the turn record;
- Monitor streams and agents launched in the turn that ends, from the transcript.

While a goal is open, a clean Stop reads idle only after the goal verdict (or the turn record, or
20 s). A proactive switch waits for any of this background work
([ADR-0063](decisions/0063-background-work-and-goal-verdict-from-the-transcript.md)):

```mermaid
flowchart LR
    prompt["prompt / wake-up\n(hook, user line)"] --> working
    stop["clean Stop hook"] --> idle["waiting / done"]
    turn["turn_duration:\npending background agents"] --> bg["+ background work\n(shown Working)"]
    launch["transcript: monitor started,\nagent launched (until it ends)"] --> bg
    idle --> bg
    stop -.->|"goal open: until the verdict,\nthe turn record or 20 s"| working
    verdict["goal verdict: not met"] --> working
    work["assistant work after the stop"] --> working
    notice["notice that asks a person"] --> attention["needs_attention"]
    attention -.->|"agent works on"| working
```

## Run metrics

A run's tokens, turns and active time are computed from its own transcript lines and its
sub-agents' transcripts, by one accumulator (`yulius-harness` `observe`), live while it runs and
again after a restart ([ADR-0070](decisions/0070-run-metrics-come-from-the-transcript.md)). Runs
recorded before that rule are counted again once: migration 23 queues every run recorded so far,
and after start, once reconciliation has ended the lost runs, a background job counts each ended
run's window of its transcript and records the outcome with the figures it replaced
([ADR-0072](decisions/0072-recount-old-runs-from-their-transcripts-once.md)):

```mermaid
flowchart LR
    mig["migration 23:
queue every run so far"] --> queue[("metrics_recounts")]
    start["start: reconcile"] --> job["recount_old_metrics
(background)"]
    queue --> job
    job -->|"ended run, transcript exists"| count["harness.recount
(spawn_blocking): its window
from its start to its end
or the next run's start"]
    count --> put[("metrics: replaced;
before/after recorded")]
    job -->|"no transcript / no lines /
live / gone"| keep[("metrics kept;
outcome recorded")]
```

## Notes

The tools rail's Notes hold a notepad per sandbox and one for the team, shared and saved while
people type ([SPEC-041](../.ai/specs/SPEC-041-here-we-want-auto-saved-notes-sandbox-wi.md),
[decision 0032](decisions/0032-shared-notes-with-revisions.md)), and one per person, "Me"
(`/api/me/notes`, row `user:<user id>`;
[SPEC-103](../.ai/specs/SPEC-103-add-me-after-team-that-is-for-user-and-n.md),
[decision 0084](decisions/0084-personal-notes-in-the-notes-table.md)). A save carries the whole
text and the revision it started from; the store writes it only when that revision is still
current, and every stored edit is announced on the event stream: a personal note's event is
addressed to its owner (`Envelope.user`), and `WS /api/events` drops it for everyone else.

```mermaid
sequenceDiagram
    participant A as Notepad (notes.js)
    participant S as Server (App::save_note)
    participant D as SQLite notes
    participant B as Teammate's notepad
    A->>S: PUT notes {body, revision: n} (600 ms after typing, blur, hide)
    S->>D: UPDATE … WHERE revision = n
    alt still at n
        D-->>S: revision n + 1
        S-->>A: Note
        S-->>B: note.updated (sandbox viewers / everyone for team / the owner for Me)
        B->>S: GET notes (nothing unsaved)
    else someone saved meanwhile
        S-->>A: 409 conflict
        A->>S: GET notes
        A-->>A: same text: adopt; else Keep mine / Use theirs
    end
```

## Agent primitives

Repository skills are APM primitives ([ADR-0016](decisions/0016-apm-for-agent-primitives.md),
[standard](engineering/primitives.md)): the sources in `.apm/skills` are deployed by `apm install`
to the harness directories Yulius already discovers, and the repository check rejects drift.

```mermaid
flowchart LR
    src[".apm/skills (source)"] -->|apm install| a[".agents/skills"]
    src -->|apm install| c[".claude/skills"]
    lock["apm.yml + apm.lock.yaml"] --> src
    a --> yulius["Yulius skill discovery (REPOSITORY_SKILL_DIRS)"]
    c --> yulius
    src -.->|check_repository.py: deployments equal sources| gate["scripts/dev check"]
```

## Runtime

- Each task run is owned by a detached `ptyhost`; the server reattaches after restart.
- Agents reach Yulius only through the internal loopback listener with per-run tokens.
- Persistence is SQLite in WAL mode; the data directory layout is in SPEC-002 section 4.
- State machines (task, attempt, switch, automation, import, onboarding, delivery, update, CI)
  are in SPEC-002 section 6; runtime contracts in section 7 and
  [engineering/runtime-contracts.md](engineering/runtime-contracts.md).
- Security and trust model: SPEC-002 section 8. Limits: section 9.

Decisions: [decisions](decisions/README.md).
