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

## Disk

A sandbox has a soft disk quota. The application prunes build caches of tasks that do not
work every 5 minutes (ended tasks, then idle ones from 90 % of the quota), removes the worktrees
of closed and cancelled tasks, and a switch checks the quota and the other start preconditions
before it ends the running session ([SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md),
[decision 0017](decisions/0017-sandbox-disk-upkeep.md)).

## Usage limits

When a subscription's usage limit cuts a turn, the task goes on as if the limit never happened
([SPEC-034](../.ai/specs/SPEC-034-in-previous-tasks-we-have-hit-a-session.md),
[decision 0027](decisions/0027-step-back-past-a-usage-limit.md)): on another subscription at
once, or on its own after the reset. The harness adapter steps the transcript back over the
limit, and Claude Code resumes the same session with `CLAUDE_CODE_RESUME_INTERRUPTED_TURN=1`, so
it redoes the lost turn by itself. Nothing is typed.

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
    H->>T: step back: drop errors and notices, relink parentUuid
    H->>N: claude --resume id (flag set, no prompt)
    N->>T: the lost turn, redone
```

## Agent feedback

Yulius has no messages for now ([SPEC-028](../.ai/specs/SPEC-028-delete-message-mechanism-for-now-until-w.md),
[decision 0023](decisions/0023-no-messages-until-delivery-is-reliable.md)): nothing is stored for
an agent, injected by a hook or typed into its session. An agent pulls what it needs: it waits
with `yulius wait` in a background shell, which Claude Code wakes it from, and reads
`ci_status` and `delivery_status`. The armed merge's outcome is kept in memory for the wait.

```mermaid
sequenceDiagram
    participant A as Agent (Claude Code)
    participant W as yulius wait (background shell)
    participant Y as Yulius (internal API)
    A->>Y: delivery_merge (checks still running: armed)
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
the request file in the sandbox home keeps the whole request
([SPEC-011](../.ai/specs/SPEC-011-automatic-dev-delivery.md),
[SPEC-025](../.ai/specs/SPEC-025-uploaded-attachements-should-be-clickabl.md),
[SPEC-030](../.ai/specs/SPEC-030-i-asked-previous-task-to-attach-images-a.md),
[decisions 0022](decisions/0022-native-task-attachments.md) and
[0026](decisions/0026-attached-images-are-pasted.md)).

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
    end
    C->>S: Stop hook of that turn (goal mode)
    S->>C: types /goal <condition>
```

## Cockpit

Every user has one plain Claude Code session per sandbox, the Cockpit
([SPEC-021](../.ai/specs/SPEC-021-in-this-bar-we-need-to-add-cockpit-at-th.md),
[decision 0020](decisions/0020-cockpit-as-a-plain-shell-session.md)). It is a sandbox shell of
kind `cockpit` that runs `claude` instead of the login shell: no hooks, settings or briefing, a
pool subscription's token, and the newest conversation resumed whenever it starts again.

It manages the project through the Yulius MCP server
([SPEC-037](../.ai/specs/SPEC-037-i-got-something-like-this-tasks-and-cock.md),
[decision 0029](decisions/0029-cockpit-manages-through-mcp.md)). Its bearer (`ck_…`) is derived
from the shell's persisted host token, and it is served only the management tools: tasks,
automations, workflows and sessions. Those tools act for a person (`ProjectCaller`): the Cockpit's
owner, or a task's creator when a task's agent calls them. `task_create` takes the same path as
New task.

```mermaid
flowchart LR
  UI["Sidebar: Cockpit"] -->|"POST /api/sandboxes/sid/cockpit"| APP["App::cockpit_open"]
  APP -->|"pool_account"| POOL[("Subscriptions")]
  APP -->|"plan_cockpit"| HP["Harness: claude [--resume id] --mcp-config yulius"]
  APP --> SH["Shell runtime (user space, repository)"]
  UI -->|"WS /api/shells/id/terminal"| SH
  SH -->|"Bearer ck_… (YULIUS_COCKPIT_TOKEN)"| MCP["/mcp: management tools"]
  AG["Task agent"] -->|"Bearer task token"| MCP
  MCP -->|"task_create"| NT["App::create_task_as (as New task)"]
  MCP -->|"automation_*"| AU["Automations"]
```

## Web UI state

The web UI is plain ES modules. Server state comes from the API and the UI event stream
(`store.js`); what only one person's browser needs stays in `localStorage`: preferences (theme,
column widths, model and effort), the notification history, and the new-task composer's drafts
([decision 0021](decisions/0021-composer-drafts-in-the-browser.md)). Sign-out removes the
notification history and the drafts.

Every drop-down menu is built by `openMenu` (`dom.js`). It stays inside the window, scrolling its
items, and a menu of 8 or more items opens with a filter box
([decision 0030](decisions/0030-menus-filter-when-long.md)).

```mermaid
flowchart LR
  C["Composer (taskview.js)"] -->|"edit, attach"| D["drafts.js (per user and sandbox)"]
  C -->|"upload"| U["POST /api/sandboxes/:id/attachments"]
  D -->|"debounced, on leave or hide"| LS[("localStorage yulius.draft.*")]
  LS -->|"after a reload"| D
  C -->|"task started: clear"| D
  SO["Sign-out"] -->|"forget"| LS
```

## Task status

A running task's activity is derived from Claude Code's own records, not guessed from the last
event ([ADR-0033](decisions/0033-task-status-follows-claude-codes-own-records.md)):

```mermaid
flowchart LR
    prompt["prompt / wake-up\n(hook, user line)"] --> working
    stop["clean Stop hook"] --> idle["waiting / done"]
    turn["turn_duration:\npending background agents"] --> bg["+ background work\n(shown Working)"]
    idle --> bg
    verdict["goal verdict: not met"] --> working
    work["assistant work after the stop"] --> working
    notice["notice that asks a person"] --> attention["needs_attention"]
    attention -.->|"agent works on"| working
```

## Notes

The tools rail's Notes hold a notepad per sandbox and one for the team, shared and saved while
people type ([SPEC-041](../.ai/specs/SPEC-041-here-we-want-auto-saved-notes-sandbox-wi.md),
[decision 0032](decisions/0032-shared-notes-with-revisions.md)). A save carries the whole text and
the revision it started from; the store writes it only when that revision is still current, and
every stored edit is announced on the event stream.

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
        S-->>B: note.updated (sandbox viewers / everyone for team)
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
