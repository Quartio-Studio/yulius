# SPEC-001: Yulius product specification

Status: accepted (reviewed against sources 2026-10-07; implementation authorized)
Date: 2026-10-07
Owner: Quartio founders (Michał Karp, Bartłomiej Nowak); implementing agent
Scope authorization: user goal of 2026-10-07 ("write the specification first … then implement
the full specified product"). Deployment is explicitly outside this request.

Companion documents:

- [SPEC-002 architecture and runtime contracts](SPEC-002-yulius-architecture.md)
- [SPEC-003 source traceability review](SPEC-003-traceability.md)

## 1. Sources and evidence method

| Code | Source | Notes |
| --- | --- | --- |
| `T mm:ss` | Oct 6 2026 product call transcript, 295 segments, Polish | All segments read. Diarization is broken (both founders labelled as one speaker); turns are inferred from content. |
| `V mm:ss` | Screen recording of the same call (37:46) | Video time equals transcript time. Used for pointing ("this panel"), reference UI and on-screen notes. |
| `WA hh:mm` | Founders' morning notes list in a group chat, scrolled on screen at V 3:45, V 23:30 and V 31:45–35:40 | The call explicitly walks this list (T 3:38 "the points we wrote this morning"; T 35:44 "that's all from WhatsApp"). Only product notes are used; personal chat is excluded. |
| `P(n)` | Michał's unfinished draft goal prompt, item *n*, on screen at V 3:38 and V 35:45 | Already written before the discussion (visible at V 3:38), so **spoken statements in the call are later and take precedence**. |
| `K` | User clarifications in the 2026-10-07 goal | Rust implementation; Product dev VM; Studio continuous; support both simply; robustness. |
| `F 2026-10-07` | Founder feedback after using the deployed instance (2026-10-07 goal: ten items, then the recovery code, five additions — agent cursor, hidden tools rail, the "unknown tab" error, link routing, HTML text with instant copying — and a simpler editor instead of per-user VS Code) | Later than every other source; supersedes them where it conflicts (class C). |
| `F 2026-10-08` | Founder decision of 2026-10-08: "prune Codex support and switching providers; we use only Claude; if Codex gets better, we'll support it" | Latest source; supersedes every Codex and cross-provider statement (class C). |
| `F 2026-10-08 UI` | Founder UI review of 2026-10-08, given item by item while using the web UI (user menu in the sidebar, named tools, wordmark, notification center, composer pickers) | Latest source for the interface; supersedes earlier layout statements where it conflicts (class C). |
| `F 2026-10-09` | Founder feedback of 2026-10-09 on the deployed instance: after admitting themself as superadmin and signing out, GitHub sign-in in the same window showed a raw "not admitted" error and, on reload, "sign-in expired"; it must work in the same window, a frequent case; later the same day: a task kept showing "Working" in the sidebar after Claude Code hit the usage limit ("You've hit your session limit · resets 10:20am"); automatic routing started new tasks on exhausted accounts; account usage showed only "HTTP 429" and "Not known yet"; Dashboard tokens did not update while tasks ran; the task terminal could not be scrolled up; finished tasks waited for someone to open their pull request; Git → a task showed no changes or commits per task; the task view looked cut on the right when another viewer controlled the terminal; in the evening: "terminal should work instantly; terminal is user's and one user should create many terminals; per user per sandbox there can be many terminals; they are different between different sandboxes and different users" (the Shell panel showed only an "Open shell" button), and "when I open Claude the font size changes 3 times and it loads for a while; make the Claude view much more user friendly"; and the sandbox browser "works terribly when we need to do something real" — render it on the user's side, use the VM only to get around iframe problems, deny recursion, keep the agent's cursor | Latest source for sign-in, usage-limit status, routing, account usage, live metrics, terminal scrolling, automatic pull requests, the task review, user terminals, how the task terminal opens and the browser panel. |
| `F 2026-10-10` | Founder feedback of 2026-10-10 on the deployed instance (`main` dfe22eb, the sandbox Chrome shown through the DOM mirror): "the browser is fucked up. It's slow as fuck — on scrolling or typing — and clicking on a captcha doesn't work. The current approach seems wrong. Fix. You can use a completely different approach because the bugs seem to be in the foundations." The use case: helping the agent through captchas and sign-ins in its own browser, then watching it work. The same day's list ("make it the final state") adds: Messages show only messages between sessions and tasks, since users write to the agent in Claude (item 2), and "bruh" at the Git page's "checks cannot be read … HTTP 403" banner (item 8); and item 10 (c)(d): task intake with TypeSafe's Jev (split detection with a Claude-proposed split, scope discipline while a task runs: new scope, dropped scope, redirect, context) and chained tasks, refined to "if the thread is consistent, it should be one task … 'ux work' and 'backend work', not 10 different tasks" and "do not overcomplicate this jev thingy, it should be just consistent"; and layout and navigation (items 1, 3, 6, 7): "make the bars resizable horizontally", "main screen with sandboxes as default view", buttons overlapping "when it's smaller", and "some space at the right"; and item 10 (b) on the Claude subscriptions: "we want to reduce management of subscriptions … we add claude subscriptions … maximally use all our resources … monitor when we'll need to buy more … pausing tasks should be gentle … paused when 98% is used and nothing is left atm, and resumed gently when the first subscription arrives … done by our harness only … invisible for the agents"; and on the terminal (item 5): another viewer's size showed in half of a wide window behind a "Take control" strip ("each user should be able to control it on their side"), Ctrl+C/Ctrl+V did not copy and paste, and file paths in Claude Code's output should open the editor as links open websites; later the same day, item 10(a): "when the task's goal is achieved and none left shell is running, the task should be marked as finished (with green 'v' icon). Atm it keeps running." | Latest source for the browser panel and for when a task is Done (TSK-014); supersedes the mirror and the live tabs as the user's view of the sandbox's browser (class C). Also the latest source for the Messages panel (COM-005) and unreadable GitHub checks (GIT-001). Source of task intake and chaining (TSK-011..013, HRN-006, STP-005). Latest source for column widths, the home screen and page layout (UI-044..UI-047). Latest source for the subscription pool, superseding priorities, the 99 % threshold and prompts sent on resume (ROT-003/004/009/010/013, class C). Latest source for the terminal's size, clipboard and file links (UI-018, UI-019). |
| `R:product`, `R:studio` | Read-only inspection of `/Users/mrfishpl/Desktop/quartio/product` and `/studio` | Structure, instructions, workflows. |

Raw sources, frame captures and research notes stay outside Git (local notes). This document
paraphrases; it does not reproduce private content.

Requirement classification used throughout:

| Class | Meaning |
| --- | --- |
| **E** | Explicit requirement stated or agreed in the sources |
| **C** | Later correction; supersedes an earlier statement (the superseded statement is cited) |
| **K** | User clarification (2026-10-07) |
| **X** | Example given in the sources; illustrative, not a binding feature |
| **D** | Explicitly deferred ("later", "in future", "not MVP") |
| **U** | Unresolved ambiguity; a default is chosen and recorded in §9 |
| **A** | Engineering assumption by the implementer; reversible; not a user requirement |

## 2. Vision

Yulius is a self-hosted harness for **"weaponized vibe-coding"** (T 3:05, P(1)): a person who is
not a professional programmer describes what they want, and an existing coding agent (Claude Code;
Codex was planned and is deferred, F 2026-10-08) builds production-quality software, while Yulius
imposes the discipline the models do not impose on their own — specs, plans with todos, documented
decisions, architecture docs, consistent project context, git hygiene and tested delivery
(T 16:27–23:13, T 34:06–35:00).

Yulius is **not a coding agent**. It is an orchestrator of existing coding agents (P(5)): every
task is exactly one native Claude Code session, shown as the agent's own terminal UI
(T 10:48–11:25). Yulius adds what the harness lacks or does inconsistently: organization-wide
accounts with seamless rotation (T 11:44–14:45), task-to-task communication
(T 13:27–13:48, T 23:31–23:53), a task step manager (T 15:15–15:43), MCP and skills managed once
(T 17:06–17:21, T 23:53–24:20, T 32:28), automations, GitHub-mirrored permissions
(T 8:41–10:25, T 31:00–31:44), and analytics (T 35:19–35:41).

Target users: a team of 2–5 people, or one person, working comfortably and consistently without
"slop" (T 36:07–36:36, P(1)). Cezar is the liked reference for concept and parts of the interface,
but Yulius needs different foundations (T 2:31–3:38, T 35:53).

Primary measure of success: one task has the context of the whole project and everything that
came before, so decisions across many sessions stay consistent (T 34:06–35:00, "probably the most
important task").

## 3. Glossary

| Term | Definition |
| --- | --- |
| Instance | One Yulius installation on one VM, bound to exactly one GitHub owner (an organization or a personal account) where projects are published (T 8:41, P(4)). |
| Superadmin | The single local administrator created at installation (T 30:22, P(3)); owns the instance and sees every sandbox (ACC-012). |
| User | A person the superadmin admits; always authenticated with GitHub (T 30:46–31:00). |
| Sandbox (project) | One GitHub repository and its isolated workspace on the instance. One sandbox = exactly one repository; many sandboxes per instance (T 8:41–10:25). UI tabs are sandboxes (T 6:42). |
| Task | Exactly one coding-agent session lineage (Claude Code) with its own worktree, branch, spec and steps (T 11:25, T 23:41). It may span several process runs (resume, account switches) but is one conversation. |
| Harness | A supported coding-agent CLI: Claude Code (T 11:28–11:32; Codex deferred, F 2026-10-08). |
| Account | An organization-connected Claude subscription login (T 11:50). |
| Step | An item of a task's Yulius-managed plan/todo list (T 15:30). |
| Skill | A `SKILL.md` playbook that the agent activates automatically by task relevance (T 24:04). |
| Automation | A schedule or trigger that creates tasks (T 24:48–25:22, T 33:35). |
| Workflow | A reusable task recipe: initial steps, instructions and gates (T 5:28, WA 13:08). |
| Delivery profile | A sandbox's branch/environment/promotion rules (T 21:12–23:13, K). |

## 4. Users and journeys

### J1 — Install an instance (operator)

1. Operator installs Yulius on a Linux VM and starts it (T 29:27, T 30:05). A one-time setup
   token is printed on the VM.
2. Operator opens the public URL, enters the setup token, sets the superadmin password. The UI
   states prominently that the password **cannot be recovered** and asks the user to store it in
   several safe places before continuing (T 30:22–30:38).
3. Superadmin connects the instance to the GitHub organization or personal account (T 8:41,
   T 31:00) and admits users by GitHub login (T 30:46).
4. Superadmin connects Claude accounts by clicking a button and completing Claude's own login
   (T 31:44–32:00, WA 13:10).

### J2 — Start a new project from a template

1. A user clicks "+" in the sandbox tab bar, chooses "Create", picks a basic template and a name
   (T 32:06–32:10, WA 13:10 "templates").
2. Yulius creates the GitHub repository under the instance owner, scaffolds the enforced
   documentation format, configures branch protection for the default delivery profile, and opens
   the new sandbox tab (T 19:00, T 32:10).

### J3 — Build a feature by vibe-coding

1. The user clicks "New task" and writes "make me an online store" (or a feature request); the task
   runs Claude Code (T 11:28, T 22:00; F 2026-10-08).
2. The center of the screen becomes that agent's native terminal (T 4:36, T 10:48–11:13).
3. Guided by Yulius' briefing and skills, the agent decides and documents the technology
   (T 20:00–20:34), writes the task spec with a plan of todos (T 17:49–18:00), and Yulius shows the
   steps and the agent's current position (T 15:30–15:43).
4. The agent implements on the task's own branch and worktree (T 23:00, T 23:41), runs tests, and
   the user delivers through PR → integration → environment → production (T 23:00–23:13).
5. If an account runs out of usage, the session continues on another Claude account without the
   user doing anything (T 12:00–13:01).

### J4 — Import an existing project (Product, Studio)

1. On their computer, a user runs `yulius import`, authenticates, and chooses how far back to import
   local Claude Code sessions, e.g. two weeks (T 26:00).
2. Yulius links the GitHub repository (it must exist), syncs local unpublished state, and imports
   sessions opened in that repository as organization tasks (T 26:00, T 27:50–28:08).
3. Yulius starts a preparation meta-task that adapts the project to the discipline; the user only
   approves or is informed it is running (T 27:00–27:44, WA 13:38).
4. Any member can open an imported session (even another person's) and continue it
   (T 28:08–28:33).

### J5 — Control work from a phone

Close the laptop; tasks keep running on the VM; log in from a phone and steer tasks
(T 28:38–29:00, WA 13:34).

### J6 — Coordinate parallel tasks

Two tasks in one sandbox see each other's purpose and status, exchange
messages and agree on scopes; each works in its own worktree (T 13:27–13:48, T 23:31–23:53).

### J7 — Daily feedback automation

A user tells an agent "create an automation that every day creates a task implementing yesterday's
customer feedback from the Quartio MCP" — the automation appears and runs daily (T 24:48–25:41,
T 33:35, WA 13:13).

### J8 — Ship Product and Studio changes (K)

- Product: task branch → PR into `quartio` → deploy to Quartio's own dev VM and verify → fast-forward
  `main` → full rollout (K, R:product).
- Studio: task branch → PR into `quartio` → fast-forward `main` immediately; customers test the live
  result (continuous development until deliberately treated otherwise) (K, R:studio).

## 5. Requirements

Each requirement has an ID, observable behavior, source, class and acceptance evidence. Acceptance
scenarios with IDs (`AC-…`) are in §10. "Must" applies to E/C/K requirements; A items are design
defaults that implementation may refine without changing user-visible intent.

### 5.1 Instance, identity and access (ACC)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| ACC-001 | Exactly one superadmin per instance, created at installation with a password chosen then. | T 30:22, P(3) | E | AC-ACC-1 |
| ACC-002 | The superadmin password can be recovered only with the instance's 6-digit recovery code (ACC-011); there is no email reset. Setup shows the password warning and the recovery code once, and requires explicit confirmation that both are stored safely. | T 30:22–30:38, P(3) "recovery code is given", F 2026-10-07 (recovery code; supersedes "no recovery code") | C | AC-ACC-1, AC-ACC-7 |
| ACC-003 | Setup is reachable only with a one-time setup token available on the VM, so a public instance cannot be claimed by a stranger. | — | A | AC-ACC-2 |
| ACC-004 | The superadmin admits users (by GitHub login); only admitted users may sign in; the superadmin can disable a user, which ends their sessions. A refused GitHub sign-in returns to the sign-in page in the same window, never to a raw error: it names the GitHub account that was refused and offers GitHub's account picker to continue with another account; reloading the page after a refusal shows the sign-in page again. | T 30:46, F 2026-10-09 | E | AC-ACC-3 |
| ACC-005 | Every user authenticates with GitHub; there is no user password. | T 30:52–31:00, WA 13:40, P(4) | E | AC-ACC-3 |
| ACC-006 | The instance connects either the organization's GitHub or a private GitHub account, and every project is a repository of that owner. | T 8:41–9:00, T 31:00, P(4) | E | AC-PRJ-1 |
| ACC-007 | Project permissions mirror GitHub repository permissions: no access → the sandbox is invisible (as if nonexistent); read → can view tasks, terminals (read-only), steps and history but not act; write → can create/continue/steer tasks and use browser/editor; admin → sandbox configuration. The superadmin is the one exception (ACC-012). | T 9:00–9:39, T 31:11–31:44, P(4), F 2026-10-09 | E | AC-ACC-4 |
| ACC-008 | Permission changes on GitHub take effect in Yulius within 5 minutes without user action, and immediately after the user signs in again; revocation closes the user's live terminal connections to that sandbox and stops its live events; signing out, an expired session or a disabled user closes every open connection (#66). | T 31:38 ("consistent") | A | AC-ACC-5 |
| ACC-009 | The superadmin may mark users as instance administrators, who may manage accounts, instance MCP servers and the instance skill catalog. Instance administration never grants repository access that GitHub does not grant (only the superadmin's ownership does, ACC-012). | — | A | AC-ACC-6 |
| ACC-011 | Recovery code: a uniformly random 6-digit code generated at setup, shown exactly once and stored only as an Argon2id hash. "Forgot password" with the right code sets a new password, ends every superadmin session and shows a fresh code once. After 3 wrong codes recovery is permanently disabled (the counter survives restarts); only a signed-in superadmin who re-enters the password can generate a new code, which replaces the old one and re-enables recovery. The UI states that the code must be kept perfectly safe. | F 2026-10-07 | C | AC-ACC-7 |
| ACC-010 | A Settings page, reached from the user menu (never the sidebar), gathers the signed-in user's account (archived tasks, CLI device approval, sign out), this browser's preferences (default model and effort for new tasks, the tools rail, notification history) and tiles into instance administration; the compact admin menu stays. | T 5:31–5:33 (supersedes P(2) "settings"), F 2026-10-08 UI (supersedes "no Settings section") | C | AC-UI-1, AC-UI-15 |
| ACC-012 | The superadmin owns the instance: they see and can open every sandbox with admin access, without a GitHub link and without asking GitHub. Every access path applies it: the sandbox list and REST routes, the event stream and terminal, shell and browser sockets (their periodic re-checks never close or filter the superadmin's), editor files, the Git page, tasks, Status and analytics. Only the superadmin: instance administrators and every other account keep GitHub-mirrored access (ACC-007, ACC-009), and an account without GitHub sees no sandbox. The UI shows the superadmin no "needs a GitHub-linked account" gate. Actions that act on GitHub go through the GitHub App installation where Yulius already acts as the App: opening a task's pull request (authored by the App, as for a ready task, GIT-001), listing and importing the owner's repositories the App can access, and Git in the superadmin's shells (a token for that repository only). Merging a pull request and releasing act as a person on GitHub: for the superadmin only that action is refused, with a message saying so (do it on GitHub, or sign in with GitHub as an admitted user); the page and session stay. | F 2026-10-09 (supersedes U-06) | C | AC-ACC-8 |

### 5.2 Sandboxes and projects (PRJ)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| PRJ-001 | One sandbox is exactly one GitHub repository; an instance hosts many isolated sandboxes on one VM. | T 9:30–10:28, T 29:07 | E | AC-PRJ-1 |
| PRJ-002 | Checking out another repository inside a sandbox is not supported; cross-project knowledge comes through MCP (e.g., Quartio MCP). | T 9:51–10:22 | E (agreed to drop the wish) | AC-PRJ-2 |
| PRJ-003 | A project is created either by importing an existing repository or by creating a new one. | T 32:06 | E | AC-PRJ-1, AC-IMP-1 |
| PRJ-004 | Creating a project offers a few basic templates; on creation Yulius creates the GitHub repository automatically and keeps the sandbox synchronized with it. | T 32:10–32:28, WA 13:10 | E | AC-PRJ-3 |
| PRJ-005 | A newly created project automatically receives the enforced documentation format (§5.8). | T 19:00 | E | AC-PRJ-3 |
| PRJ-006 | Users can switch between projects quickly via the sandbox tab bar. | WA 13:09, T 6:42 | E | AC-UI-2 |
| PRJ-007 | Sandboxes are isolated from each other: a task in one sandbox cannot read or write another sandbox's files or processes. Worktrees isolate changes, not processes; containment uses an OS boundary on the VM. | T 9:41 ("isolated sandboxes"), engineering rules | E + A | AC-NFR-6 |
| PRJ-008 | Templates (initial set): "Empty project (Yulius format only)", "Web application", "Python data project", "Rust service". Each produces the docs format and a minimal runnable skeleton with a test command. | T 32:10 ("a few basic templates") | A (selection) | AC-PRJ-3 |
| PRJ-009 | Removing a sandbox from Yulius never deletes the GitHub repository; it stops sessions, preserves an export of tasks, and requires repository admin. | — | A | AC-PRJ-4 |
| PRJ-010 | Each sandbox gets a distinct port range for development servers (exposed to agents as environment variables and in the briefing) so previews of parallel sandboxes do not collide, and the sandbox browser can open them (the reference's Web panel previewed the app under development, V 2:30). | V 2:30, T 9:41 | A | AC-PRJ-5 |

### 5.3 Workspace interface (UI)

Layout reference: the Open Mercato sandbox page (V 2:30–6:50): left Cezar navigation, center work
area, right "Web | Editor", plus a top usage bar and a "AI Coding Agent | Terminal | Logs" tab row.

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| UI-001 | Three zones: a left sidebar with navigation and tasks; a center that is the task's terminal; a right zone with Browser and Editor. | T 4:05–4:20, V 4:04–4:24, P(2) | E | AC-UI-1 |
| UI-002 | Browser and Editor are hidden by default, shown as icons that expand a panel on demand. | T 4:20–4:32, T 5:50–6:00 | E | AC-UI-1 |
| UI-003 | Everything except the left sidebar is the terminal by default, for clarity and maximum visible content. | T 4:36–4:51 | E | AC-UI-1 |
| UI-004 | The left sidebar offers: New task, Tasks (overview), Git (incl. GitHub PRs/checks), Workflows, Skills, Automations; plus Dashboard (analytics) and MCP. Active tasks appear at the bottom of the sidebar as in Cezar. | T 5:00–5:28, T 24:39, WA 13:08, P(2) | E | AC-UI-1 |
| UI-005 | The sidebar's task list shows each task's status as an icon (working, waiting for input, needs attention, switching, stopped, failed); the status badge in the task header is removed. | T 16:12 | E | AC-UI-3 |
| UI-006 | Active tasks stay in the sidebar; archived tasks are not a permanent toggle — they live in a separate "Archived tasks" page reachable from the user menu, with restore. Archiving a task remains a one-click action. | T 5:36–5:47, T 33:44–34:05, WA 13:27 | E/C | AC-ARC-1 |
| UI-007 | The large top bar (CPU/GPU/RAM/cost) and the project-name header are removed. Resource usage moves into a hidden Status panel opened by a "Status" control. | T 6:00–6:21, V 6:00–6:27 | E | AC-STA-1 |
| UI-008 | A second-level, thin tab row (no thicker than the "AI Coding Agent / Terminal / Logs" row) lists sandboxes, e.g. "Quartio Studio", "Sandbox 1", "Sandbox 2", replacing Terminal/Logs tabs. | T 6:27–6:50, V 6:33–6:44 | E | AC-UI-2 |
| UI-009 | The task header panel (Session/Changes/Commits/Files tabs, Continue/Open in/Notes/Mark unread/Pin/Archive/Delete) is removed; Files duplicates the editor. Only the step indicator survives, redesigned. | T 14:50–16:00, V 14:50–16:23 | E | AC-UI-4 |
| UI-010 | Harness/model/effort indicators belong at the bottom where commands are typed (the native agent footer), not in a header. | T 16:04 | E | AC-UI-4 |
| UI-011 | Links in the terminal and chat are clickable, including Yulius links that open a specific browser tab. | WA 13:12, T 33:29, T 8:00 | E | AC-UI-5 |
| UI-012 | The interface is usable on a phone: tasks list, status, terminal view and a quick-input bar that sends text/keys to the session. | T 29:00, WA 13:34 | E | AC-UI-6 |
| UI-013 | A small per-task action menu provides: Stop, Continue, Switch harness/account, Deliver, Feedback, Archive. | T 13:04–14:28 (switching), T 35:19 (feedback), P(5) | A (placement) | AC-UI-4 |
| UI-014 | Visual direction: dark, dense, terminal-first, following the reference's compactness; one consistent design system. | V 2:30–6:50 | A | AC-UI-1 |
| UI-015 | A general sandbox shell (contained, write permission) is available from the right rail, hidden by default; it is separate from the agent terminal and never replaces it. Opening the Shell panel shows a working terminal at once, without a button. | T 11:13 ("a separate terminal is here, of course"), V 11:12 (reference Terminal tab) vs T 6:42 (Terminal/Logs tabs replaced by sandbox tabs), F 2026-10-09 | U-11 | AC-UI-7 |
| UI-016 | Terminals (sandbox shells) belong to one user and one sandbox. A user keeps several per sandbox, as tabs (at most 8 live): opening the Shell panel shows the one used last, or a new one when the user has none there; "+" opens another in the chosen folder (the sandbox repository or the current task's worktree); tabs switch, rename (double-click or F2) and close (× or Delete). Only that user can list, see, type into, rename or close them (administrators included), and another sandbox never shows them. They keep running across page reloads and server restarts and end when closed, when their process exits, or after a day without viewers. Inside the sandbox every user is a separate user with root access: their own login name and prompt, root privileges in the sandbox (`id -u` is 0, `sudo` works), and a private home that persists across their sandboxes and sessions. | F 2026-10-07, F 2026-10-09 | C (system-directory writes U-12) | AC-UI-8, AC-UI-37 |
| UI-017 | The whole right tool rail (browser, editor, shell, messages) is hidden by default; a top-bar toggle shows it and the choice is remembered per browser. | F 2026-10-07 (supersedes the visible icon rail of UI-002) | C | AC-UI-9 |
| UI-018 | Links in terminals, messages and pages open in a new tab of the user's browser; links to the VM itself (localhost, loopback addresses) open in the sandbox browser instead, since only it can reach them. File references in terminal output (the task terminal and the user's terminals) are links too, with the same hover underline and plain click: bare or quoted relative paths, `path:line[:col]`, `path#Lline`, absolute paths inside the folder and OSC 8 `file://` links that name an existing file of the task's worktree (or the terminal's folder, below the directory the shell reports) open the built-in editor at that file and line; missing files, paths outside the folder and `.git` stay plain text. | F 2026-10-07, F 2026-10-10 | C | AC-UI-10 |
| UI-019 | Terminal and TUI text is HTML text in the user's browser, not an image: a selection is copied at once, and a double-click selects a word even when the program (e.g., Claude Code) switched on mouse reporting; the mouse can be handed to the program on request. The conversation always scrolls with the wheel or trackpad: a full-screen program that asked for the mouse (Claude Code 2.1.x draws on the alternate screen, which has no scrollback) receives the wheel turns while clicks stay with text selection; otherwise the terminal's own scrollback scrolls. Each viewer sees the terminal full size on its side: the PTY takes the size of the active viewer's window — typing or clicking into the terminal takes it at once, an active viewer takes it from an idle one (hidden, unfocused or unused for a minute), a viewer alone always has it — and a viewer that does not set the size sees that grid scaled by font size to fill its box (both ways, aspect ratio kept, centred, never cut off) with a small chip, never a banner or a "Take control" step. Copy and paste work as in a desktop terminal: ⌘C/⌘V on macOS; Ctrl+C copies a selection and interrupts the program without one; Ctrl+V, Ctrl+Shift+V and Shift+Insert paste (bracketed when the program asked for it); the right-click menu has Copy, Paste and Select all; read-only viewers cannot paste. | F 2026-10-07, F 2026-10-09, F 2026-10-10 | C | AC-UI-11 |
| UI-020 | A notification center behind a bell in the tab row collects, per user, what happened while the user looked elsewhere: tasks that wait for input, need attention or fail, browser action requests, agent messages, automation runs, account problems and instance notices. Unread entries are counted on the bell; an entry opens its task, browser tab or page; entries can be marked read or cleared. Toasts still appear. | F 2026-10-08 UI | C | AC-UI-12 |
| UI-021 | The new-task composer is a rounded prompt box with its choices along the bottom edge, like a chat input: mode, workflow, model and effort as compact pickers, then Start (Enter starts and Shift+Enter adds a line; on touch screens Enter adds a line and the button starts). Models come from a list grouped by provider (no free-text model ids); effort offers the levels the model supports (none for a model without effort); "Default" leaves either to the harness. The last model and effort are remembered per browser. | F 2026-10-08 UI | C | AC-UI-13 |
| UI-022 | Each account reads at a glance: an avatar with initials, its label, sign-in email (administrators) and plan (e.g. Pro, Max 5x, Max 20x, per provider), a state only when it is not ready, and each usage window as a horizontal row named like Claude's own usage page ("Current session", "This week") with its reset time and "N% used". Accounts are visually separate; in the Status panel each is a row that expands, accounts needing attention first and open. | F 2026-10-08 UI | C | AC-UI-14 |
| UI-023 | New task has a keyboard shortcut that works from anywhere, the terminal included: ⌘⇧O / Ctrl+Shift+O (browsers reserve ⌘N for a new window), plus ⌘N on macOS where the browser passes it (installed app, full screen); Ctrl+N stays with the terminal. The New task button shows the shortcut. | F 2026-10-08 UI | C | AC-UI-16 |
| UI-024 | The Tasks page groups tasks as Needs you → Working → Done → Stopped → Closed with the sidebar's rules (browser requests count as Needs you); each row says its state in words with the step, number and branch, and shows its pull request with checks, author and age. Filters: state tabs with counts, Author and Origin multi-select, search (`/`); the filters are kept in the page URL. | F 2026-10-08 UI | C | AC-UI-17 |
| UI-025 | The Git page explains delivery in plain language: one sentence on how the sandbox ships (from its profile); what is live in production and what is waiting to go live, with the environment steps and the release button (disabled with its reason); "Delivery needs you" rows with one reason and one action each (named apart from the task list's "Needs you"); a board of where every task is (Being built → In review → On integration, not live → Live, columns from the profile); recent deploys, verifications and commits. The release confirmation lists the tasks that go live. | F 2026-10-08 UI | C | AC-UI-18 |
| UI-026 | A task that needs the user shows a bar under its step strip that says what is needed: a browser request with its reason and an "Open browser tab" button that opens the requesting tab; a question or a wait for input with a "Reply" button that focuses the terminal (the quick input on phones). The bar floats over the top of the terminal: coming and going with every turn, it never resizes the terminal (each resize makes the agent redraw its screen). | F 2026-10-08 UI, F 2026-10-09 | C | AC-UI-19 |
| UI-027 | New and edited automations pick their schedule from presets — every day, every weekday, once a week (with the day), every hour — at a time, and the page says in words when it runs; the raw cron expression stays available under "Advanced", and typing one selects the matching preset (or Custom). | F 2026-10-08 UI | C | AC-UI-20 |
| UI-028 | The notification center filters All / Unread and marks a single entry read from its row; with "Desktop notifications" switched on in Settings (asking the browser's permission only then), new entries also reach the operating system while the tab is in the background, and clicking one opens its task. | F 2026-10-08 UI | C | AC-UI-21 |
| UI-029 | Every keyboard shortcut is listed in one sheet, opened with ⌘/ (Ctrl+/ outside the terminal, where it belongs to the shell) or from Settings; shortcuts are shown in the platform's own key names (⌘⇧O on macOS, Ctrl+Shift+O elsewhere) wherever the UI mentions them. | F 2026-10-08 UI | C | AC-UI-22 |
| UI-030 | The task view's strip shows the task's pull request with its checks (passed, failed, running), which opens the delivery details, and the next delivery step as a button when it is to merge or, for a closed task, to open the pull request; members with read access see the chip without the button. | F 2026-10-08 UI | C | AC-UI-23 |
| UI-031 | On the Git page a task card opens the task's review instead of its session: Changes (the diff against the integration branch, uncommitted files included, with a file tree, unified or split layout and line wrapping), Commits and Branch (position against the integration branch, head, pull request, compare on GitHub), with "Go to task" at the top. Changes and Commits read `GET /api/tasks/{tid}/changes` and `/commits` (docs/api.md): the task worktree — committed, uncommitted and untracked work, symbolic links never followed — against the merge base with the integration branch, bounded (`truncated`), and the branch's commits with their size; a server without them answers 404 and the review shows the Branch tab alone. | F 2026-10-08 UI, F 2026-10-09 | C | AC-UI-24 |
| UI-032 | The Tasks page is a one-line table (task, status in words, branch, changes, pull request, author, started, updated; fewer columns as it narrows), and a row opens the task's overview instead of its session: state and what it needs, start, running and active time, last change, plan progress, changes (linking to the review), tokens, the plan's steps, details (author, origin, mode, account, models, turns, branch, pull request, delivery, spec, worktree) and its sessions, with "Review changes" and "Go to task" at the top. Group heads in the sidebar and on the Tasks page are bands of their own. | F 2026-10-08 UI | C | AC-UI-25 |
| UI-033 | A skill opens on its own page in a Notion-like Markdown editor: the text shows formatted while you write (headings, lists, to-dos, quotes, code, dividers, bold, italic, code, links), the frontmatter shows as properties, "/" opens the block menu and Markdown shortcuts ("# ", "- ", "1. ", "[] ", "> ", "```") turn a block into that kind; the Markdown source is one click away; Save (⌘S) writes the file and leaving with unsaved changes asks first. The Open Mercato pack and skills the user may not edit open read-only, saying why. A repository skill is a file in the sandbox repository, read and saved with the built-in editor's files API (a save carries the read's sha256, so a file changed meanwhile is a conflict, not an overwrite); instance skills and the Open Mercato pack are read-only. | F 2026-10-08 UI | C | AC-UI-26 |
| UI-034 | A workflow opens in a builder on its own page, made for clicking rather than typing: a new one starts from a template (Feature, Bug fix, Quick change, UI change, Research, Blank); steps are added by clicking or dragging them from a library (or typing your own), reordered by dragging (Alt+↑/↓ from the keyboard), renamed in place and removed; the delivery gates are switches; instructions are added with one-click snippets; a summary shows what a task started with it gets. Instance workflows are read-only for non-administrators. | F 2026-10-08 UI | C | AC-UI-28 |
| UI-035 | Skills has New skill: a page where you name the skill (turned into a valid skill name, its file path shown), say when Claude Code should use it, pick a template (Checklist, Conventions, Review, Voice and tone, Blank) and write it in the block editor; the frontmatter is written for you. It lives in this repository (`.agents/skills/<name>/SKILL.md`), created with the built-in editor's files API (folder, then file; an existing skill is refused, never overwritten). | F 2026-10-08 UI | C | AC-UI-29 |
| UI-036 | Tasks has no search field: the header says "Press / to search" (a search button on touch screens), and "/" opens a Spotlight-style search over the page: results as you type (by title, number, branch or author; most recently updated first when empty), ↑/↓ to choose, Enter opens the task's overview, ⌘/Ctrl+Enter its session, Esc or a click outside closes; the last row filters the list by the text, shown as a removable chip and kept in the URL (`?q=`). | F 2026-10-08 UI | C | AC-UI-30 |
| UI-037 | Page filters are tabs with counts, not drop-downs (Skills by source, MCP by scope; empty choices hide); every other list to choose from opens a styled list matching the app's menus (surface, border, radius, accent check) where the browser supports a customizable select, the native control elsewhere. | F 2026-10-08 UI | C | AC-UI-31 |
| UI-038 | An MCP server is added and edited on its own page with as little typing as possible: pick a known server (Quartio, GitHub, Apify, Context7, Stripe, Supabase; those attached show Added) and only its secret is left, or paste what the server's docs give — a URL, a command line (leading KEY=value become its environment), `claude mcp add …` or `mcpServers` JSON — and the name, connection and secrets fill themselves; remote servers take a URL and a token (sent as Authorization: Bearer), local ones one command line and their variables; stored secrets show by name only and are replaced only on request, never dropped silently; after Add the server opens and is tested at once. | F 2026-10-08 UI | C | AC-UI-32 |
| UI-039 | An automation opens in a builder on its own page, made for clicking: a new one starts from a recipe (Daily feedback, Dependency updates, Fix failing checks, Triage issues, Weekly summary, Blank) that fills its name, prompt and time; the schedule is chosen with buttons — once a day on chosen days (with Every day, Weekdays, Weekends) at a time picked from chips or typed, or every hour at a chosen minute — and the page says in words when it runs next; the time zone is the browser's unless changed; cron stays under Advanced (other shapes are kept and shown as custom). The workflow is a chip; Run now, the on/off switch and the recent runs are on the same page. | F 2026-10-08 UI | C | AC-UI-33 |
| UI-040 | The Dashboard reads like a modern product dashboard: one strip of key figures (tasks with a daily trend line, median duration, active agent time, tokens, account switches, feedback correct with its split), each with its change against the previous period of the same length — green or red only where a direction is better, neutral otherwise — and one line of context; the tasks-per-day chart as the main card, with the average dashed, weekends shaded, today marked as partial, dates as "Sep 14" (weekday names for 7 days), a hover band with the day and count, and a Table toggle; outcomes as one split bar with a legend of counts and shares; tokens by model and account and where tasks start as bar lists with shares. Export is one menu (CSV, JSON). | F 2026-10-08 UI | C | AC-UI-34 |
| UI-041 | Brand direction Roma ("Dym i kreda"), the default look (accepted 2026-10-08; the brand lab keeps it at four contrast levels, `?lab=roma` and `roma-1`…`roma-3`, to experiment): a matte smoky black with one lighter intermediate dark and chalk white, no accent hue, states in Roman pigments, both themes; Instrument Sans, Tenor Sans (titles, wordmark) and IBM Plex Mono, self-hosted; a fine grain and a soft light over large surfaces; the mark in an arched niche over the composer with smoke rising in it (WebGPU via the vendored `shaders` library, telemetry removed; only when the niche shows, the browser has a GPU adapter and the person asked for neither reduced motion nor data saving; paused in hidden tabs). Every repository contrast pair passes in both themes. | F 2026-10-08 UI | C | AC-UI-35 |
| UI-042 | The tools rail holds its items: each tool is a fully rounded pill inside the rail with the same space on both sides, and a count (at most "99+") or attention dot stays inside its pill; the open tool — one at a time — shows the accent fill, an accent bar and a heavier label, clearly apart from the quieter hover fill; keyboard focus shows the focus ring. An open panel never pushes the rail out of the window: when the window narrows the panel gives way (down to 300 px, the centre keeping 360 px where it fits) and grows back to its width when the window widens. On phones the rail's icons sit centred in the top bar, and the sandbox list gives way to them rather than covering them. | F 2026-10-09 ("it's a little broken") | C | AC-UI-36 |
| UI-043 | Opening a task shows its Claude Code terminal once, at its final size and font: no font-size change, reflow or layout shift after it appears (the size travels with the connection, the server's first screen is drawn at it, after the agent's redraw, and the typeface is loaded first); until then a calm placeholder of the terminal's colour, never text at another size; the keyboard is in the terminal. The step strip says the session's state in words and colour, as the sidebar does (working, your turn, needs your action, done — goal achieved or with its pull request, TSK-014 —, usage limit with the reset time, stopped, failed); a blank terminal says why (queued, starting, waiting for a usage reset); connection trouble floats over the terminal with Retry. A reload revalidates the UI's files instead of downloading them again. | F 2026-10-09 | C | AC-UI-38 |
| AC-UI-39 | `?mock=1` at 1440 px: dragging the sidebar's edge 80 px to the right widens it by 80 px; ←/→ on its focused splitter change the width and update its value; after a reload the same user sees the dragged width, a double-click restores the default. The explorer and the tool panel resize the same way and keep their widths after a reload. Narrowing the window to 1024 px with the rail and a panel open keeps the rail and at least 360 px of centre in the window. At 390 px no splitter is shown and the drawer opens. | ui |
| AC-UI-40 | `?mock=1` opens `/` on the home screen: a card per sandbox (studio, product, sandbox-3) with its repository, counts and last activity; a card's New task opens that sandbox's composer with the prompt focused, Open opens the sandbox; `#/s/studio/t/12` still opens task #12; from a sandbox the logo, the All sandboxes tab and ⌘⇧H / Ctrl+Shift+H return home; the shortcut sheet lists it; with no sandbox the home screen offers Import and Create; no WCAG A/AA violation. | ui |
| AC-UI-41 | `?mock=1` at 768 and 390 px on every page of UI-046: no two header, toolbar or list controls' bounding boxes intersect and the page does not scroll sideways; at 1440 px with the rail and a panel open the Tasks state tabs wrap instead of running past the page, and on task #12 with Editor, Browser or Shell open (1440 and 1024 px) the step strip's and the panel head's parts do not intersect. | ui |
| AC-UI-42 | `?mock=1` at 1920 px on the pages of UI-046 and the workflow builder: the content's left and right margins inside the page differ by at most 2 px. | ui |
| UI-044 | The sidebar, the editor's explorer and the tool panel are resizable by dragging their edge: a visible but quiet grip with a resize cursor that is also a focusable separator with an accessible name and value (←/→ resize, Shift for bigger steps, Home/End for the narrowest and widest); a double-click resets the default width. Widths are kept per user in this browser and clamped so the rail and the centre (360 px where it fits) never leave the window: when room runs out the sidebar gives way first (down to 200 px), then the panel (down to 300 px), and both grow back when room returns (UI-042). Phones keep the drawer and full-screen panels, without splitters. | F 2026-10-10 ("make the bars resizable horizontally") | C | AC-UI-39 |
| UI-045 | The default landing view (`/`) is a home screen listing every sandbox the user can see, in the tab order, as cards: name, repository, running / needs you / queued counts, last activity, and quick actions to open the sandbox or start a new task in it; with no sandbox it offers Import repository and Create from template (and the GitHub notes of UX2-17). Deep links to a sandbox, a task or a page open it directly. The sandbox tabs stay for quick switching and begin with an "All sandboxes" tab; the logo and ⌘⇧H (Ctrl+Shift+H elsewhere) go home, listed in the shortcut sheet (UI-029). | F 2026-10-10 ("main screen with sandboxes as default view"; supersedes the redirect to the last used sandbox) | C | AC-UI-40 |
| UI-046 | Page headers, toolbars and lists never overlap: titles, actions, filter tabs and chips wrap onto new lines, list columns give way as the list narrows (measured on the list, so an open tool panel counts) and a narrow task list stacks its status under the title; a task's step strip and the tool panel's head give way part by part (the panel keeps its title); no page scrolls sideways, from 320 px up, on Tasks, Git, Workflows, Skills, Automations, MCP, Dashboard, Settings and the home screen, in both themes. | F 2026-10-10 ("when it's smaller buttons are overlapping") | C | AC-UI-41 |
| UI-047 | Every top-level page uses the width it has the same way: its content is centred, up to the page's maximum width, with equal margins, never left-aligned with an empty band on the right. | F 2026-10-10 ("here is some space at the right") | C | AC-UI-42 |

### 5.4 Tasks and native sessions (TSK, HRN)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| TSK-001 | One task is exactly one Claude Code session. | T 11:25, P(5), F 2026-10-08 | C | AC-TSK-1 |
| TSK-002 | The harness is chosen when creating the task; different tasks may use different harnesses. | T 11:28, P(5) | E | AC-TSK-1 |
| TSK-003 | Only Claude Code is supported (Codex was planned and is deferred until it improves); the design keeps a harness port so another harness can be added later. | T 11:32, F 2026-10-08 (supersedes "Claude Code and Codex") | C + D | AC-TSK-1 |
| HRN-001 | The center shows the agent's **native terminal interface** (the real TUI), exactly like a local Claude Code session — not a re-rendered chat. | T 10:48–11:20, P(5) "seriously TUI" | E | AC-TSK-2 |
| HRN-002 | The task terminal is dedicated to the agent: the user cannot drop to a shell; leaving the agent session (e.g., `/exit`) closes the task. A separate general terminal is not part of the task view. When a task closes because the user left the session, the UI returns to the graphical task list, closes the task view and removes the task from the Active tasks list (it remains under Tasks with Continue). | T 11:06–11:20, F 2026-10-07 | E | AC-TSK-3 |
| HRN-003 | Native commands work: `/model`, `/effort`, `/fast` and other slash commands of the harness. | T 14:18 | E | AC-TSK-2 |
| HRN-004 | Harness features that conflict with "one session = one task" (e.g., Claude switching to another session, `/resume` pickers) are handled: Yulius detects a session-id change and either adopts it into the same task lineage or prevents it, without corrupting another task. | T 13:04–13:27 | E | AC-TSK-4 |
| HRN-005 | Task sessions show Claude Code's own footer as in a local session, with a status line Yulius configures (Claude Code `statusLine` in the task's Claude settings): the sandbox's repository and the task's branch ("→ studio git:(feat/checkout-api)"), above Claude Code's mode line (contained runs: "bypass permissions on", with background shells and agents). Proposed: the harness does not set the status line yet; the web mock shows the intended footer. | F 2026-10-08 UI | C | AC-UI-27 |
| HRN-006 | The native session keeps its input, with one gate: Claude Code's `UserPromptSubmit` hook lets Yulius see each submitted prompt first (TSK-013). A held prompt never reaches the agent: Claude Code shows the person the reason and keeps the text in its input box; slash commands are never held. | F 2026-10-10 | E | AC-TSK-12 |
| TSK-004 | Tasks keep running when the browser window closes; the user can come back from any device. | T 28:38–29:00, P(1) "doesn't stop when the website is closed" | E | AC-NFR-1 |
| TSK-005 | Each task (each fix) has its own git worktree and branch. | T 23:41 | E | AC-TSK-5 |
| TSK-006 | Several users may watch the same task; one person types at a time (the active viewer — the last to type or click into the terminal — controls terminal size; UI-019). | T 28:00–28:33 (shared org sessions), F 2026-10-10 | A | AC-TSK-6 |
| TSK-007 | A stopped or closed task can be continued (resumed) by any member with write permission. | T 28:08–28:33 | E | AC-TSK-7 |
| TSK-008 | A per-instance concurrency limit queues extra tasks visibly instead of overloading the VM. | robustness (K) | A | AC-NFR-3 |
| TSK-009 | A write user can delete a task permanently, not only archive it. Deletion requires confirmation, refuses live tasks (stop first) and removes the task's messages, steps, metrics, feedback, switch history, run records and directories, and its local worktree and branch; remote branches and pull requests are untouched. | F 2026-10-07 | C | AC-TSK-8 |
| TSK-011 | Split detection on a new task: a request that mixes clearly separate workstreams (for example user-interface work and backend work) is gently offered as several tasks; consistency beats granularity, so one coherent thread stays one task whatever its numbered items, and when unsure there is no offer. Jev decides "one task" or "several" with a conservative threshold (prompts under 40 words are never checked); only then a headless Claude run on a connected account proposes the **smallest** split — normally 2, at most 4 tasks, grouped by area, titled "Area: what", each carrying its items word for word — and a proposal with more tasks, one task per item or a dropped item is never shown. The composer shows "This looks like N separate tasks." with editable titles and **Create N tasks (in parallel)** · **Create as a chain** · **Keep as one task**. The check runs after Enter with a bounded wait (≤ 4 s); without Jev, when it is slow, or without a usable proposal the task is created as written. | F 2026-10-10 | E | AC-TSK-10 |
| TSK-012 | Chained tasks: a task can be created to run **after another task** of its sandbox ("Run after #N" in the composer, "Create as a chain", and the held-prompt choices). It waits as "Queued after #N" and starts by itself when its predecessor is **Done** (a finished turn and a pull request merged, or open and tested — the sidebar's rule). When the predecessor stops, fails, is cancelled, closes before it is done or is deleted, the chained task goes to **Needs you** with the reason and never starts by itself; continuing the predecessor puts it back to waiting. **Start now** and **Cancel** are always available; the chain shows on both tasks. A chained task starts from its predecessor's branch until that is merged. | F 2026-10-10 | E | AC-TSK-11 |
| TSK-013 | Scope discipline while a task runs: a prompt a person types into the session is checked before the agent sees it. Context passes untouched; **new scope** is held with **Start in parallel** · **Run after this task**; a prompt that **drops scope** is held with "Skip the affected steps and run the new direction after this task"; a **redirect** is held with "Cancel this task, revert its changes, and start a new task" (explicit confirmation; the work is kept on a branch, nothing is deleted); **Send anyway** delivers any held prompt unchanged and records the override. Slash commands, short answers, Claude Code's own turns and anything uncertain (low confidence, Jev unavailable) pass. Scenario analysis: SPEC-002 ADR-16. | F 2026-10-10 | E | AC-TSK-12 |
| TSK-010 | Goal mode: a new task chooses between **Goal** (default) and **Prompt**. In goal mode the task prompt becomes the session's goal through Claude Code's native `/goal` command, typed once the TUI is ready, so the agent keeps working until the goal is met; a prompt longer than Claude Code accepts as a goal (500 characters) is sent as the first message and the goal requires everything it asks for to be done and verified. Prompt mode sends it as a normal first message. The mode is stored with the task, shown in the task view, and re-applied when the session is continued (also on another account). | F 2026-10-07 | C | AC-TSK-9 |
| TSK-014 | A task is **Done** when its agent has finished: its last turn ended cleanly (no Stop-hook block, no permission prompt or browser request waiting), no background shell it started is still running (Claude Code's "· 1 shell", `run_in_background`), and the work is evidently complete — Claude Code recorded the task's goal as achieved ("✓ Goal achieved"), or its pull request is tested (GIT-001). Done shows a green ✓ in the sidebar's Done group, on Tasks, in the task header and in notifications. Failing checks put it under Needs you; a background shell still running keeps it Working with "1 background shell still running" until the shell ends. Plan steps are shown, not required. The session stays attached and idle: typing in it makes the task Working again and the achieved goal no longer counts, so it is Done again only with a newly achieved goal (`/goal …`) or a tested pull request. The automatic pull request still opens at the clean Stop (GIT-001). A Yulius restart neither reopens a finished turn nor loses a goal achieved while Yulius was down. | F 2026-10-10 | C | AC-TSK-13 |

### 5.5 Accounts and rotation (ROT)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| ROT-001 | The organization can connect many Claude accounts (e.g., 15); the design allows more providers in future. | T 11:50–11:58, T 14:00, P(5), F 2026-10-08 | C | AC-ROT-1 |
| ROT-002 | Connecting an account is a button followed by the provider's own sign-in; no long CLI typing. | T 31:44–32:00, WA 13:10 | E | AC-ROT-1 |
| ROT-003 | When an account's usage is exhausted — or proactively at 98 % of any window binding the task (5-hour, weekly, or the weekly cap of the task's model) — the task moves to the best other Claude subscription at its next turn boundary by native resume (mid-turn only when the CLI reports a limit), underneath and smoothly; neither the user nor the agent should notice. An account seen at its limit stays limited until its reset, so new tasks are not routed to it. | T 11:44–12:18, P(5), F 2026-10-09, F 2026-10-10 (98 %, supersedes ~99 %) | C | AC-ROT-2 |
| ROT-004 | The Claude subscriptions are one self-managing pool with no manual priority or order: new and resumed tasks go to the subscription with the best use-it-or-lose-it score (weekly headroom per hour until that window resets, shared by the tasks already on it), so capacity that expires soon is spent first and concurrent tasks spread over the pool; ties go to the capacity that expires soonest; known headroom comes before unknown usage, and a window whose reset passed counts as fresh. A task never changes harness or provider. | T 12:23, P(5), F 2026-10-07, F 2026-10-08, F 2026-10-10 (supersedes admin priority) | C (reset semantics U-03) | AC-ROT-3, AC-ROT-9, AC-ROT-14 |
| ROT-005 | Withdrawn (F 2026-10-08): no provider switches, so no session translation. Originally: a provider switch translates the session (Claude ↔ Codex). | T 12:23–13:01, T 14:28–14:45, P(5); F 2026-10-08 | C (withdrawn) | — |
| ROT-006 | Withdrawn with ROT-005 (F 2026-10-08): translation quality as a tested capability. | T 14:28–14:45; F 2026-10-08 | C (withdrawn) | — |
| ROT-007 | Users can switch a task to another account manually. | P(5), T 28:20, F 2026-10-08 | C | AC-ROT-5 |
| ROT-008 | The account orchestrator respects per-task model/effort settings and preserves them across switches where the target supports them. | T 14:00–14:18 | E | AC-ROT-5 |
| ROT-009 | Subscription management is add (the provider's own sign-in), see usage, remove — nothing else to manage: no priority, order or on/off switch (F 2026-10-10); a subscription whose credential broke offers its sign-in again. The accounts section shows each account's usage: provider-reported windows (5-hour, weekly, and per-model weekly caps such as Opus when reported) with percent used and reset time when known, Yulius-observed tokens of the last 24 hours and 7 days, and the tasks using it; unknown values are shown as unknown. Usage refreshes in the background (every 10 minutes, the first round 2 minutes after start). A limit the CLI reports shows as its window at 100 % with the reset time even when the provider's usage request fails; a throttled usage request (HTTP 429) honours `Retry-After`, keeps the last known values and reads as a note, not as a broken account. | WA 13:12, T 33:29, F 2026-10-07, F 2026-10-09, F 2026-10-10 (supersedes disable and priority) | C | AC-ROT-1, AC-ROT-7, AC-ROT-13 |
| ROT-010 | When no subscription has capacity for a task (every one at 98 % or limited in a window binding it), the harness pauses the task gently at its turn boundary — the agent is held at its Stop or prompt hook, a task without a process waits to start — and never sends Claude anything about pausing: the agent sees nothing. A paused task reads "Paused: waiting for subscription capacity (resets ~14:20)", is not under Needs you, survives a Yulius restart, and resumes by itself, oldest first and staggered, on the subscription with capacity (by native resume when it differs) when a window resets or a subscription is added. The only text the harness sends on its own after a pause is a neutral "Continue." for a turn the CLI itself interrupted with a usage limit. A usage limit is recognised from the CLI's own limit record (Claude Code 2.1.292: `apiError: "usage_limit_reached"` with `resetsAt`), whatever its wording, so the task never keeps showing "Working" while the agent is stopped at a limit. | —, F 2026-10-09, F 2026-10-10 (supersedes "needs attention" and resume prompts) | C | AC-ROT-6, AC-ROT-10, AC-ROT-11 |
| ROT-011 | Unknown usage is never treated as zero usage; every switch decision is recorded with its inputs for audit. | engineering rules EVT-001 | A | AC-ROT-2 |
| ROT-012 | A task never changes harness: rotation, Continue and manual switches stay on Claude accounts; there is no harness proposal, approval or setting for it. Rotating from one Claude account to the next stays automatic (proactively at the 98 % threshold from refreshed usage, reactively on a usage limit). | F 2026-10-07, F 2026-10-08 (supersedes approval of harness changes), F 2026-10-10 | C | AC-ROT-8 |
| ROT-013 | Monitoring of the pool: the Accounts page and the Dashboard show, per subscription, both windows with use and reset times; pool totals; the burn rate over the last 24 hours and 7 days; when the pool runs out this week at the current rate; the task-hours paused for lack of capacity in the last 7 days; and a plain recommendation — "Add about N subscriptions to stop pausing" or "Capacity is sufficient" — by the formula in SPEC-002 ADR-17 §2.4. | F 2026-10-10 | E | AC-ROT-12, AC-ROT-13 |

### 5.6 Task step manager (STP)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| STP-001 | Yulius has its own step/todo manager per task, independent of the harness: the plan's subtasks, their status, the current step, and what remains. | T 15:15–15:43 | E | AC-STP-1 |
| STP-002 | Steps are auditable: every change records time, actor (agent/task, user) and note. | T 15:30 "very auditable" | E | AC-STP-1 |
| STP-003 | The step indicator replaces Cezar's "Do the task — step 1 of 1" bar in a better form: a slim strip above the terminal showing current step and progress, expandable to the full list. | T 15:11, V 15:11 | E (form A) | AC-STP-2 |
| STP-004 | The step list stays in sync with the task spec's Plan checklist (spec is the durable source in the repo). | T 17:49–18:00 | A | AC-STP-3 |
| STP-005 | A step a person rules out is **skipped**: a finished state (it counts as done for progress and delivery) recorded with who ruled it out and why; the agent is told not to work on it (TSK-013). | F 2026-10-10 | E | AC-TSK-12 |

### 5.7 Communication and shared context (COM)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| COM-001 | Tasks can see previous and parallel tasks of their sandbox. | T 13:27 | E | AC-COM-1 |
| COM-002 | Tasks can communicate through a Yulius-level protocol (any task can message another). | T 13:33–13:48, T 23:47, WA 13:05 | E | AC-COM-2 |
| COM-003 | Tasks can agree on scopes (who works on what) to avoid conflicts. | T 23:31–23:41 | E | AC-COM-3 |
| COM-004 | Every task starts with the context of the whole project: project summary, architecture, decisions, recent changes, parallel tasks. | T 34:38–35:00, WA 13:44 | E | AC-COM-4 |
| COM-005 | A message to an idle task can wake it so negotiation proceeds without the user. Users see messages in the UI, and an agent tells or asks people something with `message_send` `to="people"`: it shows in the task's Messages and in their notifications (#46). The task's Messages panel shows only messages between sessions and tasks and agents' messages to people, with its unread count; it has no composer and there is no API to post a person's message to an agent: people talk to the agent in its Claude Code terminal, and a message in which the agent asks people something offers "Reply in the terminal", which opens the task and focuses its terminal (the quick input on phones) (#110). | —, F 2026-10-09, F 2026-10-10 (no user → agent messages) | C | AC-COM-2 |

### 5.8 Discipline (DSC)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| DSC-001 | Yulius enforces docs kept in the repository in one specific format; templates of formats are a future option. | T 17:35, T 21:40, WA 13:35 | E (+ D for templates) | AC-DSC-1 |
| DSC-002 | One task = one spec of what the task should do; for a feature the spec is an implementation plan that makes everything auditable. | T 17:49–18:00, T 21:02 | E | AC-DSC-2 |
| DSC-003 | The plan must contain todos and specific sections; when missing, Yulius prompts the agent that something is missing and the agent must fix it. | T 18:00 | E | AC-DSC-3 |
| DSC-004 | The repository keeps an architecture diagram and other needed diagrams. | T 18:00 | E | AC-DSC-1 |
| DSC-005 | Documentation and backward-looking decisions live next to code as Markdown; Yulius enforces the format. | T 18:00 | E | AC-DSC-1 |
| DSC-006 | When creating or converting a product, technology decisions are made by the agent (analysis, choice, justification) and documented; the user should not need to choose. | T 20:00–20:34 | E | AC-DSC-4 |
| DSC-007 | Git discipline: no direct pushes to production; feature branches and PRs; a consistent model of basic git hygiene. | T 21:12–21:29, WA 13:39 | E | AC-DSC-5 |
| DSC-008 | Delivery path per feature: spec → new branch → implementation per spec → tests, QA → merge to dev → merge to main; testing at every stage; deployment handled within the flow. | T 22:27–23:13 | E | AC-DLV-1 |
| DSC-009 | A project summary and summaries of recent changes keep decisions consistent across sessions. | T 34:18–34:49, WA 13:44 | E | AC-COM-4 |
| DSC-010 | Imported projects are not assumed to comply: a preparation meta-task adapts them first, needing minimal user attention (approve, or be informed). It asks whether to reverse-engineer the existing project into docs. | T 27:00–27:44, WA 13:38 | E | AC-IMP-4 |
| DSC-011 | The implementing team may research documentation best practice; for now one task = one spec as above. | T 20:40–21:02 | E | — |
| DSC-012 | Production-grade defaults for non-programmers: the agent is guided to tests, error handling and conventions without the user knowing frameworks. | T 19:15–19:59, T 22:00 | E | AC-DSC-4 |

### 5.9 Skills (SKL)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| SKL-001 | Skills activate automatically when relevant; users do not need slash commands ("make me this design" invokes the design skill). | T 23:53–24:18, WA 13:11 | E | AC-SKL-1 |
| SKL-002 | Skills from the organization's existing repositories (repository `.agents/skills`, `.claude/skills`) are used this way. | T 24:20 | E | AC-SKL-1 |
| SKL-003 | A curated subset of Open Mercato skills useful for code discipline is included; not all of them. | T 33:00–33:22, WA 13:11/13:17 | E (selection A, §7.3) | AC-SKL-2 |
| SKL-004 | One skill catalog serves every Claude Code session ("skill orchestration"). | WA 13:07 | E | AC-SKL-1 |
| SKL-005 | A Skills page lists skills, their source and enabled state, and shows their content. | T 5:28, T 24:39 | E | AC-SKL-3 |

### 5.10 MCP (MCP)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| MCP-001 | MCP servers are attached once in Yulius and automatically available to every Claude Code session. | T 17:06–17:21, T 32:28–32:55, WA 13:11, P(7) | E | AC-MCP-1 |
| MCP-002 | Examples to support: Quartio MCP (product and customer knowledge), Apify MCP (e.g., scraping X), any other MCP. | T 17:21, T 32:39 | X (any MCP must work) | AC-MCP-1 |
| MCP-003 | MCP secrets are stored encrypted and never shown back; the Yulius tool server is always attached. | — | A | AC-MCP-2 |

### 5.11 Automations and workflows (AUT)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| AUT-001 | Users can create automations, e.g., every day create a task that implements the last day's feedback. | T 24:48–25:22, WA 13:13 | E (daily feedback = X) | AC-AUT-1 |
| AUT-002 | Automations and workflows can be created by asking the agent inside a task. | T 25:22–25:41, T 33:35, WA 13:13 | E | AC-AUT-2 |
| AUT-003 | Automations support cron schedules. | WA 13:13, T 33:35 | E | AC-AUT-1 |
| AUT-004 | Workflows tab: reusable task recipes (initial steps, instructions, gates) selectable at task creation. | T 5:28, T 24:39, P(2) | E (content A) | AC-AUT-3 |
| AUT-005 | Automation runs are deduplicated and never create duplicate tasks for one scheduled time, across restarts. | robustness (K) | A | AC-AUT-4 |

### 5.12 Git and GitHub management (GIT, DLV)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| GIT-001 | Git can be managed from Yulius: branches/worktrees per task, changes, PRs and CI checks. When a task is ready for delivery — valid spec, every step done, work committed, commits ahead of the integration branch — Yulius pushes its branch and opens the pull request itself when the agent's turn ends (authored by the task's creator; the agent never has to ask for it); delivery stages follow GitHub in the background, so the Git board moves when a PR is merged; a finished task with a tested pull request — open with checks green (or none required), or merged — is **Done** (its own group in the sidebar and on Tasks; the one Done rule is TSK-014); while its checks run it is "In review" (Done when its goal is achieved), and failing checks put it under Needs you. Merging and promotion stay human (GIT-002). Checks come from every source the GitHub App may read: check runs, commit statuses and, when check runs cannot be read, the head's Actions workflow runs. A source the App lacks the permission for is said once per repository, in plain words with the permission it needs, as one quiet line for instance administrators with a link to the App's permission settings that they can dismiss for the session, never as a warning on every visit. Checks that cannot be read are unknown and never count as passed: a merge or release that requires checks waits and says why, and the task is not Done (#111). | T 24:39, WA 13:08, F 2026-10-09, F 2026-10-10 (unreadable checks) | E | AC-GIT-1 |
| GIT-002 | Merges and promotions are human actions (or an explicit project policy), never inferred from model text. | engineering rules SEC-003 | A | AC-GIT-2 |
| DLV-001 | Each sandbox has a delivery profile: integration branch, production branch, merge and promotion method, environments with deploy and verification steps. | T 23:00–23:13, K | E + K | AC-DLV-1 |
| DLV-002 | New projects default to "standard": feature → PR into `dev` → (dev environment) → promotion PR `dev` → `main`. | T 23:09–23:11 | E | AC-DLV-1 |
| DLV-003 | Product profile: feature → PR into `quartio` → deploy and verify on Quartio's own dev VM → fast-forward `main` → full rollout. Deployment runs only on an explicit user action per run. | K, R:product | K | AC-DLV-2 |
| DLV-004 | Studio profile: feature → PR into `quartio` → fast-forward `main` immediately (continuous; customers test live) until the profile is deliberately changed. | K, R:studio | K | AC-DLV-3 |
| DLV-005 | Profiles stay simple: a profile is a short `yulius.toml` section; switching Studio to a production-gated profile is one change. | K ("support both simply") | K | AC-DLV-3 |

### 5.13 Browser (BRW)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| BRW-001 | A real, general browser like Chrome: any address can be typed. | T 6:54–7:00, WA 13:02 | E | AC-BRW-1 |
| BRW-002 | One browser per sandbox, shared by that sandbox's users and agents (logins such as AWS persist per sandbox). | T 7:21–7:26 (supersedes T 7:00 "global, whole VM" and WA 13:04 "per user") | C | AC-BRW-1 |
| BRW-003 | Agents control the same browser as fully as the native Claude browser plugin. | T 7:07, WA 13:04 | E | AC-BRW-2 |
| BRW-004 | Tab management; when an action is requested from the user (e.g., AWS login), the user can jump straight to that tab; the agent also writes a clickable "open this tab" link in the session. A tab that no longer exists (e.g., after a browser restart) is replaced by the current tab list, not reported as an error. | T 7:26–8:07, F 2026-10-07 ("unknown tab") | E ("nice to have", T 7:26) | AC-BRW-3 |
| BRW-005 | The browser panel is **the sandbox's Chrome itself** — the browser agents drive (BRW-002/003) — streamed to the user as live video of the real, headful browser, with the user's mouse, keyboard and touch delivered to it as real input: every tab, every site and every frame (cross-origin iframes, canvases, captchas such as reCAPTCHA and Cloudflare Turnstile, sign-in popups) works as in a local browser, and the page does not report itself as automated (`navigator.webdriver` is false). Scrolling and typing feel immediate (BRW-009). The panel shows the Chrome's tabs, an address bar, back, forward and reload; new windows and popups (OAuth) appear in the view and are usable. Text typed in any layout arrives as typed (including Polish letters, IME and emoji); paste sends the user's clipboard, copy brings the page's selection to the user's clipboard, and ⌘ works as Ctrl for editing shortcuts on macOS. The viewport is shared by the sandbox's viewers, and a panel sized by someone else says so. A browser without video decoding gets a labelled lower-frame-rate image mode. A sandbox dev server can also open live in a window of its own (the preview origins of ADR-14, where the server has them). | F 2026-10-07, F 2026-10-09, F 2026-10-10 (supersedes the mirror and the live tabs) | C | AC-BRW-4, AC-BRW-9, AC-BRW-10 |
| BRW-006 | A new tab opens the sandbox Chrome's welcome page: the sandbox name and its listening dev-server ports as links. Yulius's own ports are never listed. | F 2026-10-07, F 2026-10-09, F 2026-10-10 | C | AC-BRW-5 |
| BRW-007 | When an agent clicks, types, opens or navigates in the sandbox Chrome, viewers see it: a labelled cursor (task number and harness) at the click position with a click ripple over the streamed page, or a short notice for navigation, like the official Claude browser plugins; the agent's tab is marked in the strip, and an agent acting in a tab the user is not looking at is announced with "Show". | F 2026-10-07, F 2026-10-09 ("we still need that cursor thingy"), F 2026-10-10 | C | AC-BRW-6 |
| BRW-008 | No recursion: Yulius never opens inside itself. The sandbox Chrome refuses Yulius's addresses (asked directly or through a page's link or redirect), the preview proxy refuses Yulius's own ports and Yulius itself behind any port, the welcome page does not list Yulius's ports, and the Yulius UI refuses to render inside a frame. A page on a live preview can never use the user's Yulius session. | F 2026-10-09 ("deny recurrence … it'll explode") | C | AC-BRW-8, AC-BRW-7 |
| BRW-009 | The streamed browser is fast: scrolling a long page shows at least 30 and normally 60 frames per second, a key press or click shows in the view within about 100 ms on the server side, a slow viewer only drops to the next keyframe (never slows the browser or other viewers), and nothing is encoded while nobody watches. | F 2026-10-10 ("slow as fuck on scrolling or typing") | C | AC-BRW-12 |
| BRW-010 | The browser's display is private to its sandbox's viewers: agents and users' contained shells cannot see or drive any display (only the viewer connection, with write access, streams it or sends input to it), and one sandbox can never see or drive another's. | F 2026-10-10 (security of the new design) | C | AC-BRW-11 |

### 5.14 Editor (EDT)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| EDT-001 | The editor is a simple, VS Code-styled editor built into Yulius (not a full VS Code), hidden by default: an explorer, tabs and VS Code's editor component (Monaco) with syntax colors for popular languages, type-aware suggestions for JavaScript/TypeScript, JSON, CSS and HTML, and keyword and project-word suggestions for other popular languages; new file, new folder, delete and save. It is meant for quick changes and reviews. | T 8:21–8:31, T 5:56, WA 13:09, F 2026-10-07 ("not VS Code but much simpler … without 90% of the features"; supersedes "normal VS Code") | C | AC-EDT-1 |
| EDT-002 | The editor opens the sandbox (task worktree selectable); access requires write permission. | — | A | AC-EDT-1 |
| EDT-003 | Every user gets a clean editor view of their own: open files and unsaved text live only in that user's browser tab and are neither stored nor shared; saving writes the file into the sandbox. A file changed on disk since it was opened (for example by an agent) is reloaded when unchanged in the editor and never overwritten silently. Git in the user's shells authenticates as the user's own GitHub account. | F 2026-10-07 (per-user VS Code profiles dropped by the founder) | C | AC-EDT-2 |
| EDT-004 | The editor works end to end in a real browser: it lists the folder, opens a file with syntax colors and suggestions, and saves an edit. | F 2026-10-07 ("coding editor still doesn't work") | C | AC-EDT-3 |
| EDT-005 | The user chooses whether long lines wrap: one switch for every open file, from the editor's toolbar (a toggle that shows whether wrapping is on) or with ⌥Z / Alt+Z as in VS Code while the editor has focus, never typing a character; it is listed in the shortcut sheet. Off by default (as before); the choice is kept per user in the browser. | F 2026-10-09 ("In editor i should be able to select whether break lines or not") | C | AC-EDT-4 |

### 5.15 Import and migration (IMP)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| IMP-001 | A CLI `yulius import` imports an existing local repository; the user provides credentials; local state is synchronized (rsync-like, resumable). | T 26:00, WA 13:09 | E | AC-IMP-1 |
| IMP-002 | Every project must have a GitHub repository; import enforces it and enforces permissions. | T 25:45–26:00 | E | AC-IMP-2 |
| IMP-003 | Import brings Claude Code sessions from a user-chosen period (prompted, e.g., two weeks), only those opened in that repository. | T 26:00, WA 13:09 | E | AC-IMP-3 |
| IMP-004 | Imported sessions become organization tasks, not personal ones; any member can open another person's imported session and continue it. | T 27:50–28:33, F 2026-10-08 | C | AC-IMP-5 |
| IMP-005 | Preparation meta-task after import (see DSC-010). | T 27:00 | E | AC-IMP-4 |
| IMP-006 | Import is idempotent: re-running imports only new sessions and changed state. | robustness (K) | A | AC-IMP-6 |

### 5.16 Analytics (ANL)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| ANL-001 | Background collection per task: duration, model(s), tokens consumed, accounts used, switches. Collection is live: a run's tokens, turns and active agent time (the time its turns took) are stored after each turn and at least every 30 s while it runs, not only when it ends, and an open Dashboard refreshes itself. | T 35:19–35:38, F 2026-10-09 | E | AC-ANL-1 |
| ANL-002 | Users can give feedback on a task: was it done correctly (yes/no + comment). | T 35:19 | E | AC-ANL-2 |
| ANL-003 | Full analytics for later analysis: dashboard and export. | T 35:38–35:41 ("mega important") | E | AC-ANL-3 |

### 5.17 Operations and deployment (OPS)

| ID | Requirement | Source | Class | Acceptance |
| --- | --- | --- | --- | --- |
| OPS-001 | MVP deployment target is a VM (Linux); Quartio's own space at `julius.quartio.ai` / `yulius.quartio.ai`. | T 29:27, T 30:05, P(3) | E (domain U-01) | AC-OPS-1 (deployment unverified, out of scope) |
| OPS-002 | Server and UI are separable so the orchestrator runs on a VM while a desktop app (e.g., Electron) provides the frontend. | T 29:34–29:53 | E (desktop app D) | AC-OPS-2 |
| OPS-003 | Running the server on a local computer and migrating such an instance to a VM are later goals. | T 29:19–30:00 | D | — |
| OPS-004 | Yulius is open source. | P(3) | U-02 | — |
| OPS-005 | All durable state lives in one data directory to make backup and future migration simple. | T 30:00 (migration later) | A | AC-OPS-3 |

### 5.18 Non-functional robustness (NFR) — from K

| ID | Requirement | Acceptance |
| --- | --- | --- |
| NFR-001 | Sessions survive browser disconnects, Yulius server restarts and upgrades; a VM reboot stops sessions but never loses task history and lets users continue them. | AC-NFR-1, AC-NFR-2 |
| NFR-002 | Concurrent work: many tasks across sandboxes run in parallel with bounded resources; one slow viewer never stalls a session. | AC-NFR-3, AC-NFR-4 |
| NFR-003 | Cancellation stops the whole agent process tree, confirmed reaped, before capacity is released; exactly one terminal outcome is recorded per run. | AC-NFR-5 |
| NFR-004 | Reconnect: terminals resynchronize to the exact current screen after network loss; no duplicated or reordered input. | AC-NFR-4 |
| NFR-005 | Recovery: on restart Yulius reconciles every recorded live session with reality and never spawns a duplicate for a record that says "running". | AC-NFR-2 |
| NFR-006 | Isolation between sandboxes and between tasks (worktrees); secrets never in argv, logs or the repository. | AC-NFR-6, AC-NFR-7 |

## 6. Interface specification

### 6.1 Global layout (desktop ≥ 1024 px)

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ [Quartio Studio] [Product] [Sandbox 3] [+]            (thin tab row) Status ▾ │ ≤ 36 px
├────────────┬──────────────────────────────────────────────────────────┬──────┤
│ + New task │ ▸ Step 3/7 · Implement checkout API     ▾ (step strip)   │ 🌐   │
│ Tasks      ├──────────────────────────────────────────────────────────┤ </>  │
│ Git        │                                                          │ ✉ 2  │
│ Workflows  │          native Claude Code terminal (xterm)             │      │
│ Skills     │                                                          │      │
│ Automations│                                                          │      │
│ MCP        │                                                          │      │
│ Dashboard  │                                                          │      │
│ ─────────  │                                                          │      │
│ ● checkout │                                                          │      │
│ ◐ 3D model │                                                          │      │
│ ! login    │                                                          │      │
│ ─────────  │                                                          │      │
│ ☺ M. Karp  │                                                          │      │
└────────────┴──────────────────────────────────────────────────────────┴──────┘
```

- Sandbox tab row: an All sandboxes tab (home), then the accessible sandboxes in user-defined order; overflow scrolls; "+" opens
  Create/Import. Right side: `Status` (opens the Status panel), the notification bell with its
  unread count (UI-020) and the tools toggle (UI-017).
- Home (`/`, the default view): every sandbox as a card with its counts, last activity, Open and New task (UI-045); the
  logo, the All sandboxes tab and ⌘⇧H return to it.
- Left sidebar (300 px, 200 px below 1024 px; resizable, UI-044): New task, navigation, then the active task list grouped by what the
  user must do — "Needs you" (waiting for input, needs attention, a browser request), "Working"
  (working, starting, queued, waiting for a free slot, switching account) and "Stopped" (stopped,
  crashed, failed; collapsible). Each row shows a distinct SVG state icon, the title, unread
  messages, and a second line in words with the step and relative time (e.g. "Working · step 4/7 ·
  now"); the harness glyph is not shown while Claude Code is the only harness. Its foot shows the signed-in
  user (avatar and name), which opens the user menu (Archived tasks, CLI access, Sign out; admin
  entries: Users, Accounts, GitHub, Instance MCP, Instance skills).
- Center: the selected task's terminal fills all remaining space. A one-line step strip sits above
  it (collapsible). No other header.
- Right rail (160 px; icons only on phones): Browser, Editor, Shell, Messages, each shown with
  its icon and name so a newcomer recognizes them. Clicking expands a resizable panel (default 45%
  width; UI-044); clicking again collapses. Default collapsed (UI-002); the whole rail is hidden until the
  tools toggle shows it (UI-017). The left sidebar stays while a panel is open (developers keep their
  tasks in view; F 2026-10-08 UI): the panel takes its width from the centre, which keeps at least 360 px.
- Without a selected task the center shows the new-task composer (UI-021): a rounded prompt box
  whose bottom edge holds the task's choices as compact pickers — Goal / Prompt mode, workflow,
  model and effort — and a round Start button (tasks run Claude Code). The model is chosen from a
  list grouped by provider (never typed), and its effort levels follow the model.

### 6.2 Pages

| Page | Content |
| --- | --- |
| Tasks | All non-archived tasks of the sandbox, grouped Needs you / Working / Stopped / Closed: state in words, step, number, branch, pull request with checks, author, last activity; state tabs with counts, Author/Origin filters and search kept in the URL (UI-024). |
| Git | Delivery at a glance (UI-025): how the sandbox ships; live and waiting to go live with environment steps and release; Needs you; a board of where every task is with PRs and checks; activity. Branch statistics and the stage track are in each task's Deliver panel (Open PR, Merge, environment step, Release). |
| Workflows | List workflows; each opens in a builder (UI-034): templates, steps to click or drag, gates as switches, instruction snippets. |
| Skills | Catalog (repository skills, instance skills, Open Mercato pack) with source, enable toggle, content view. |
| Automations | List with schedules in words, Run now and the switch; each opens in a builder (UI-039): recipes, days and times as buttons, next run in words, cron under Advanced, run history. |
| MCP | Sandbox MCP servers + inherited instance servers; test from the list; each server opens on its own page (UI-038): pick a known server or paste its URL, command or JSON; secrets write-only. |
| Dashboard | Analytics: tasks over time, durations, tokens by model/harness/account, switches, feedback ratio; export; the subscription pool for the whole instance (ROT-013). |
| Status (panel) | VM CPU/RAM/disk/load; per-sandbox sessions and disk; accounts usage bars and resets. |
| Archived tasks | Archived tasks across accessible sandboxes; restore; open read-only transcript. |
| Admin | Users (admit/disable/admin flag), Accounts (capacity and forecast; add via login terminal, usage, remove — ROT-009, ROT-013), GitHub connection, instance MCP/skills. |

### 6.3 Task actions

Action menu (kebab in the step strip and right-click in the sidebar): Stop · Continue · Switch
account · Deliver · Feedback · Archive ·
Delete permanently (TSK-009, confirmation required); a task waiting in a chain offers Start now
and Cancel instead of Stop (TSK-012). The new-task form has a Goal / Prompt mode switch with Goal
selected (TSK-010), a "Run after" choice (TSK-012) and the split card (TSK-011). Read-only users see the terminal with input
disabled and a "Read-only (GitHub: read)" banner.

### 6.4 Phone (< 768 px)

Sandbox tabs become a dropdown; sidebar becomes a drawer; terminal is full-screen with a bottom
quick-input bar (text field + Send, keys: Esc, Ctrl-C, Tab, ↑, ↓, Enter); right panels become
full-screen overlays.

## 7. Behavior details

### 7.1 Task lifecycle (user-visible)

`(chained →) queued → starting → running ⇄ (working | waiting | needs attention | done) → switching →
running … → closed | stopped | failed`, with `archived` as a flag on non-live tasks; `chained`
waits for another task (TSK-012); `waiting_capacity` ("Paused", ROT-010) is entered from
starting or running when no subscription has capacity and left by itself; `done` is a running
task whose goal is achieved and whose turn ended (TSK-014), its session still open. Leaving the agent
session closes the task (HRN-002); Continue resumes the same session lineage (TSK-007). Exact
states, owners, exit triggers and timeouts are in SPEC-002 §6.

### 7.2 Enforced documentation format (DSC)

New projects get, and imported projects are prepared to have:

```text
AGENTS.md                         agent entry point (rules, commands, layout)
CLAUDE.md                         pointer/mirror of AGENTS.md
yulius.toml                       Yulius project contract: docs paths, commands, delivery profile
docs/product.md                   project summary: purpose, users, scope, current state
docs/architecture.md              architecture with at least one Mermaid diagram
docs/decisions/NNNN-<slug>.md     ADRs: Status, Context, Decision, Alternatives, Consequences
docs/specs/YYYY-MM-DD-<slug>.md   one spec per task
docs/changes.md                   reverse-chronological summary of delivered changes
```

A task spec must contain frontmatter (`yulius-task`, `status`) and the sections Goal, Context,
Requirements, Plan (Markdown checklist, ≥ 1 item), Acceptance criteria, Test plan, Decisions, and —
by completion — Evidence. Paths are configurable per project in `yulius.toml` (imported projects
keep their conventions, e.g. Studio's single `knowledge/decisions.md`), but the required content is
fixed (DSC-001 "one format"). Enforcement points: task start (briefing), every agent turn end
(stop gate when code changed without a valid spec, bounded retries), and delivery (PR gate).

### 7.3 Open Mercato skill selection (SKL-003)

Upstream: `open-mercato/skills` (MIT) pinned at `3fc5a1fbd84d90c6f052f3c97cb1e54d2660eb22`.
Most of the 42 skills drive a tracker/PR pipeline configured by `.ai/agentic.config.json`. Included
(code discipline for non-programmers): `om-brainstorm`, `om-spec-writing`, `om-code-review`,
`om-check-and-commit`, `om-root-cause`, `om-fix`, `om-verify-in-repo`, `om-ux-shape`. Excluded for
now: tracker/PR-autopilot chain (`om-auto-*`, `om-*-pr*`, `om-merge-buddy`, `om-review-prs`,
`om-backlog`, issue tools), product-discovery and UX-pipeline setup skills, and meta skills
(`om-create-skill`, `om-apply-upgrade-notes`, `om-setup-*`), because Yulius itself provides task
orchestration, delivery and step tracking. When the pack is enabled for a sandbox, Yulius writes a
compatible `.ai/agentic.config.json` from `yulius.toml` if absent (base branch, validation
commands, GitHub tracker). Attribution and license are kept with the vendored files.

### 7.4 Subscription pool (ROT-003/004/010)

All connected Claude subscriptions form one pool; nobody ranks them (F 2026-10-10). A candidate is
a signed-in subscription below 98 % in every window binding the task — 5-hour, weekly, and the
weekly cap of an Opus or Sonnet task — with no usage limit in effect. Placement (new task,
Continue, a paused task starting): known headroom of at least 5 points first, then less, then
unknown usage; within that, the highest use-it-or-lose-it score — weekly headroom ÷ hours to its
weekly reset ÷ (tasks already on the subscription + 1); ties go to the capacity that expires
soonest. At a turn boundary a task stays while its subscription is a candidate, else moves to the
best other one by native resume; a usage limit the CLI reports moves it at once. With no candidate
the task pauses (ROT-010) until the earliest reset or a new subscription, and resumes oldest first,
staggered, in place when its own subscription recovered. A task never moves to another harness or
provider (ROT-012). The analysis and formula are SPEC-002 ADR-17.

### 7.5 Import (IMP)

`yulius login --server <url>` uses a device-style approval in the web UI. `yulius import [path]`
detects the GitHub remote, verifies or creates the sandbox (repo admin required to add a project),
uploads unpublished commits as a Git bundle and uncommitted changes as a snapshot (chunked,
checksummed, resumable), asks for the session window (default 14 days) unless `--since` is given,
lists the matching Claude Code sessions for confirmation, uploads them, and prints the sandbox URL.
Nothing is pushed to GitHub by import.

## 8. Explicit non-goals and deferred items

| Item | Source | Class |
| --- | --- | --- |
| Codex, provider switching and session translation (until Codex improves) | F 2026-10-08 (supersedes T 11:28, T 12:23–14:45) | D |
| Other harnesses (OpenCode, Pi, …) | T 11:32 | D |
| More account providers | T 11:58 | D |
| Format templates beyond the one enforced format | T 21:40 | D |
| Local (non-VM) server, instance migration to VM | T 29:19–30:00 | D |
| Desktop (Electron) app | T 29:40 | D (server/UI separation now) |
| Email-based password recovery (the recovery code exists, ACC-011) | T 30:22 | D |
| Multiple repositories in one sandbox | T 9:51–10:22 | dropped |
| General Settings section | T 5:31, F 2026-10-08 UI | a Settings page behind the user menu (ACC-010) |
| Deploying this instance (yulius.quartio.ai) | goal | out of scope of this request |

## 9. Ambiguities and decisions taken

| ID | Ambiguity | Evidence | Default chosen | Revisit trigger |
| --- | --- | --- | --- | --- |
| U-01 | Domain `julius.quartio.ai` (spoken, T 30:05) vs `yulius.quartio.ai` (P(3)). | conflicting spellings | Configurable public URL; docs use `yulius.quartio.ai` | Founder confirms domain |
| U-02 | "The product is open source" appears only in the draft prompt; Quartio repositories must be private; this repository is private. | P(3), R:studio | Keep repository private; license file stays MIT; publishing is a founder decision | Founders decide to publish |
| U-03 | "Limit resets": time-window resets vs purchasable/manual reset entitlements (chat context mentions an "additional reset"). | T 12:23, P(5) | Model both; time-window resets are used automatically; manual reset entitlements are surfaced to admins (no verified provider API to trigger them) | A provider exposes a reset API |
| U-04 | "comments też nie, files" (T 15:49) — "comments" likely "Commits". | V 15:49 shows Commits tab | Remove Commits/Changes/Files tabs from task view; Git page keeps commits | — |
| U-05 | Who manages accounts and instance MCP/skills. | not stated | Superadmin and instance admins (ACC-009) | Founders specify roles |
| U-06 | Whether the superadmin can see project content without GitHub. | not stated | Resolved by F 2026-10-09: yes, the superadmin owns the instance and sees every sandbox with admin access (ACC-012); the earlier default ("no; content access needs a GitHub-linked user") is superseded | — |
| U-07 | "whatsapp" note (WA 13:36) has no explanation in the call. | WA 13:36 only | No WhatsApp feature; Quartio MCP can provide WhatsApp knowledge | Founders explain |
| U-08 | Imported project: may other tasks start before preparation finishes? | T 27:00 "before we start a task…" | First task is the preparation task; other tasks blocked until its spec/docs land, with an admin "skip for now" override | Founders prefer otherwise |
| U-09 | Agents' autonomy level (approval prompts). | Cezar "Autonomous" toggle (V 5:04); vibe-coding intent | Full-auto only inside an OS sandbox; harness default approvals otherwise | — |
| U-10 | Browser control parity "1:1 like native plugins" — exact tool set. | WA 13:04 | Yulius browser tools via MCP + raw CDP endpoint for CDP-capable MCPs | — |
| U-11 | Is a general shell terminal still wanted? T 6:42 replaced the reference's Terminal/Logs tabs with sandbox tabs; later at T 11:13 (V 11:12, clicking the reference Terminal tab) the speaker says a separate terminal obviously exists, but the center must be the agent. | T 6:42, T 11:13, V 11:12 | Keep a hidden-by-default sandbox shell in the right rail (UI-015); no Terminal/Logs tabs | Founders say otherwise |
| U-12 | "Every user gets root access": root inside an unprivileged user namespace cannot write host system directories, and overlay mounts in user namespaces are refused on the Ubuntu 24.04 host (AppArmor userns restriction; verified 2026-10-07). | F 2026-10-07 | Users are uid 0 with `sudo` in their sandbox; homes, the sandbox and `/tmp` are writable; system packages are installed by the operator; per-user tools install into the home (e.g. `npm -g` with a home prefix, `pip --user`, `cargo install`) | A writable system layer becomes available (newer bubblewrap with `--overlay`, or a container runtime) |

## 10. Acceptance scenarios

Each scenario is observable; evidence type is noted (unit = pure tests; int = integration tests
with fakes; ui = browser verification; live = opt-in live provider smoke; deploy = deployed
instance — out of scope).

| ID | Scenario | Evidence |
| --- | --- | --- |
| AC-ACC-1 | Setup with valid token creates the superadmin only after the user confirms that the password and the recovery code are stored safely; a second setup attempt is refused. | int, ui |
| AC-ACC-2 | Setup without/with a wrong token is refused; token is single-use. | int |
| AC-ACC-3 | An admitted GitHub user signs in via OAuth; a non-admitted GitHub user is refused and redirected to `/signin` with the refused login; a reused or expired sign-in, a denied authorization and a disabled user also land on `/signin` with their reason; "another account" starts GitHub with `prompt=select_account`; disabling a user ends their sessions and WebSockets. | int (fake GitHub) |
| AC-ACC-4 | With fake GitHub permissions none/read/write/admin, API and UI expose exactly the allowed operations; invisible sandboxes return 404. | int, ui |
| AC-ACC-5 | Downgrading a permission in fake GitHub takes effect within the cache TTL and closes write-capable terminal sockets. | int |
| AC-ACC-6 | Instance admin can manage accounts but sees no sandbox without GitHub access. | int |
| AC-ACC-7 | Setup returns a 6-digit recovery code once; recovery with it sets a new password, revokes sessions and issues a new code; three wrong codes disable recovery permanently (also after restart) until a signed-in superadmin regenerates the code with the password. | int, ui |
| AC-ACC-8 | The superadmin without a GitHub link, with GitHub unreachable, lists every sandbox with `admin` access, opens one and reaches its tasks, messages, files, tools, Status and analytics; no permission is cached for it; its task terminal (writable) and event stream stay open and keep carrying the sandbox past the permission cache and several re-checks; it opens a shell whose Git gets the App's token for that repository only; it lists and imports the App's repositories; it opens a task's pull request through the App, while merging and releasing answer 403 with a message (never 401). An instance administrator or other account without GitHub sees no sandbox (every scoped route answers like a missing one) and a GitHub user sees only mirrored sandboxes. In the UI, the superadmin without a link sees every sandbox and no GitHub gate; an instance admin without one sees "No sandboxes yet" and the gate. | int, ui (mock) |
| AC-UI-8 | Two users of one sandbox: each sees only their own shells (the other's are not listed; attaching to, renaming or closing them is 404), and a shell is listed only in its own sandbox and cannot be renamed or closed through another; in a contained shell `whoami` is the user's login, `id -u` is 0, `sudo id -u` prints 0, and a file written to `~` is in that user's persistent home and invisible to the other user. | int (Linux CI for containment) |
| AC-UI-9 | The workspace opens with the tools rail hidden; the top-bar toggle shows it. | live (Chrome) |
| AC-UI-10 | Localhost and loopback links are classified for the sandbox browser, other links for a new tab. In the task terminal a bare path, `path:line:col` in backticks, a name in parentheses, an absolute path with `#L`, and an OSC 8 `file://` link of existing worktree files are links (hover) and a click opens the editor at the file, line and column; a missing file, `../` above the worktree, `.git` and an OSC 8 link outside stay plain text; a web link stays a web link; a file name a shell prints opens too; the path forms resolve as specified (unit). | live (Chrome), ui (mock) |
| AC-UI-11 | The real terminal view fed output that enables mouse tracking renders rows as HTML (no canvas), keeps the mouse for selection, and a double-click selects a word; on the alternate screen a wheel turn reaches the program as a wheel report in its encoding (the mock's full-screen session scrolls its transcript and back). A viewer that does not control the terminal (another window of 90×30) sees the whole grid filling its box in one dimension, centred, never cut off, with a chip and no banner; a click takes the size (its own grid and font). Two real windows of different sizes: the active one sets the PTY's size, the other fills its box; a click in the other moves the size; when it goes idle the active one gets it back. Ctrl+C with a selection copies it and the agent keeps running; without one it interrupts the agent; the platform's paste key types the clipboard; the right-click menu has Copy, Paste and Select all; a read-only viewer's paste is refused; the key table per platform (unit). | live (Chrome), ui (mock) |
| AC-UI-12 | A task entering "waiting for input" and a browser action request each add an unread entry to the bell (not for the task being watched); clicking an entry opens its task or tab and marks it read; the list survives a reload. | ui |
| AC-UI-13 | Picking a model, effort, workflow and mode in the composer sends exactly those values (`model` as the model id, no `effort` for a model without levels) and none when "Default" is chosen; no free-text model field exists. | ui |
| AC-UI-14 | With accounts in every state, the Status panel lists those needing attention first and open, the rest collapsed with their current-session bar; expanding one shows "Current session" and "This week" rows with reset time and percent used; email and plan appear when reported. Evidence so far is the mock only: the server does not report email/plan yet. | ui |
| AC-UI-15 | The user menu has icons and a Settings entry; `/settings` shows Your account, Preferences (default model/effort shared with the composer, tools rail, notification history) and, for administrators, Administration tiles. | ui |
| AC-UI-16 | With the task terminal focused, ⌘⇧O opens the sandbox's composer with the cursor in the prompt and the terminal does not receive the key. | ui |
| AC-UI-17 | A task waiting for input is under Needs you on both the sidebar and the Tasks page; opening the Tasks page with `?author=anna-q` shows only anna-q's tasks and the Author chip reads "Author: anna-q". | ui |
| AC-UI-18 | For a sandbox whose integration environment is not yet verified, the Git page shows the waiting tasks and a disabled release button saying "Unlocks when <environment> is verified"; after the environment is marked verified, the button is enabled and its confirmation lists those tasks. | ui |
| AC-UI-19 | When an agent requests a browser action for its task, the task view shows the reason, and "Open browser tab" opens the browser panel on the requesting tab. | ui |
| AC-UI-20 | Choosing "Once a week", Wednesday, 17:30 writes the cron "30 17 * * 3" and shows "Every Wednesday at 17:30"; typing "0 8 * * 1-5" selects "Every weekday" at 08:00. | ui |
| AC-UI-21 | With desktop notifications on and permission granted, an agent question arriving while the app is unfocused creates one OS notification; with the app focused it creates none; "Unread" lists only unread entries and a row's "Mark read" removes it from that list. | ui |
| AC-UI-22 | ⌘/ (Ctrl+/) opens the shortcut sheet and pressing it again closes it; on Windows and Linux the Tasks page and sidebar show "Ctrl+Shift+O", not "⌘⇧O". | ui |
| AC-UI-23 | A task whose pull request is open with passing checks shows "#<n> ✓" and "Merge…" in its strip; with failing checks it shows "#<n> ✗" and no Merge; a task that crashed or failed to start shows no "Open pull request…". | ui |
| AC-UI-24 | Clicking a Git board card opens /s/<sandbox>/git/t/<n> with the Changes tab: a file tree with per-file counts and the diff in unified layout; Split shows removals beside additions; a file in the tree scrolls to its diff; "Go to task" opens the task; the tab is kept in the URL. | ui |
| AC-UI-25 | Clicking a task on the Tasks page opens /s/<sandbox>/tasks/<n> showing its start date, running time, plan progress, changes and sessions; "Go to task" opens its session and "Review changes" its Git review. | ui |
| AC-UI-26 | Opening a skill shows its headings, lists and code formatted; typing "/" then choosing Heading 2, and "- " at a block's start, create those blocks; ⌘S saves and a reload shows the same Markdown; an untouched skill reads back byte for byte; an Open Mercato skill is read-only. | ui |
| AC-UI-27 | A running task's terminal ends with the input line between two rules, then "→ <repository> git:(<task branch>)", then the mode line ("bypass permissions on" in a contained run). | ui |
| AC-UI-28 | New workflow → Bug fix → clicking a library step adds it and drops it from the library; dragging step 1 below the last moves it; Alt+↑ moves the focused step; Save creates the workflow and the list shows its steps in that order, its gates and description. | ui |
| AC-UI-29 | New skill → typing "Release Notes!" shows `.agents/skills/release-notes/SKILL.md`; Create without saying when to use it shows an inline error; with it, the skill is created with `name` and `description` frontmatter and opens in its editor. | ui |
| AC-UI-30 | On Tasks, "/" opens the search with the focus in it; typing "check" lists the matching tasks; ↓ then Enter opens the second; "12" then ⌘Enter opens task #12's session; the last row sets `?q=check` and a “check” chip, and the list shows only matching tasks. | ui |
| AC-UI-31 | Skills shows "All 5 · Repository 2 · Instance 1 · Open Mercato 2" as tabs and Repository filters to two; in the automation dialog the Workflow list opens as a styled panel with the chosen item checked. | ui |
| AC-UI-32 | Add server → GitHub fills `github` and its URL and focuses the token; pasting `claude mcp add supabase -e SUPABASE_ACCESS_TOKEN=tok -- npx -y @supabase/mcp-server-supabase@latest` fills a local server named supabase with that variable; Add opens it and shows the test result with its tools; Apify shows `APIFY_TOKEN ••••` until Replace the values. | ui |
| AC-UI-33 | New automation → Dependency updates reads "Every Monday at 08:00" with the next run; turning on Wednesday and Friday reads "Mon, Wed and Fri at 08:00" (cron `0 8 * * 1,3,5`); Every hour → :30 gives `30 * * * *`; Create shows it in the list with that schedule in words. | ui |
| AC-UI-34 | Dashboard → each key figure shows a change chip against the previous 30 days; hovering a column shows its day and count; Table lists every day; 7 days labels the axis with weekday names; Outcomes shows one split bar and a legend with shares. | ui |
| AC-UI-35 | `?mock=1` shows the smoky black and chalk look in both themes with every contrast pair passing; the composer shows the mark in an arched niche; with WebGPU the niche has smoke, without a GPU adapter no shader code is downloaded and the niche stays static; nothing is sent to shaders.com. | ui |
| AC-UI-36 | `?mock=1` with the tools rail shown: with Editor open every rail item and Messages' unread count lie inside the rail and only Editor is pressed; opening Messages presses only it; no WCAG A/AA violation. With a panel open, narrowing the window to 1040 px keeps the rail inside the window (before the fix its right 20 px were cut off), and at 1000 px the centre keeps at least 300 px. At 390 px the rail's icons are centred in the top bar and no top-bar control reaches under them. | ui |
| AC-UI-37 | A user opens several terminals in one sandbox ("Terminal 1", "Terminal 2", …; a ninth live one is refused), renames one and closes one (gone from the list at once, its process ended); after a server restart the same terminals are listed with their names, screens and processes (a shell variable set before is still set); the Shell panel opens on a working terminal without a click, "+" adds a tab, tabs switch, rename and close, and another sandbox shows its own terminals. | int, ui (mock), live (Chrome) |
| AC-UI-38 | In headless Chrome over an 80 ms connection, with the fake agent drawing full screen at the size it started with: opening the task (nothing cached) and reloading it each show the terminal with zero visible font-size, grid or layout changes after the first frame, the terminal typeface loaded at that frame, no resize sent after connecting, and the keyboard in the terminal; the writer's size is in the server's first state. | live (Chrome), int |
| AC-TSK-8 | Deleting a stopped task removes its rows, run directories and worktree; deleting a live task is refused; read users cannot delete. | int |
| AC-TSK-10 | A request shaped like the founder's 10-item goal (UI fixes plus backend features) gets a 2-task proposal (UX, backend), not 10; a numbered single-thread request ("1. add field, 2. migrate, 3. show in UI") gets no card; a proposal with more than 4 tasks or one task per item is rejected and a single task means "keep one"; Jev down, no Jev key or a slow check creates the task as written. The composer card creates the tasks in parallel, as a chain, or keeps one task. | unit, int (fake Jev and Claude), ui (mock) |
| AC-TSK-11 | A task created after another waits as `chained` and both tasks show the link; it starts when the predecessor is Done (finished turn, merged pull request); a predecessor that crashes, is stopped or deleted leaves it in Needs you with the reason, also after the background sweep; continuing the predecessor clears the reason; Start now and Cancel work; chain links and held prompts survive a reopen. | int, ui (mock) |
| AC-TSK-12 | Through the `UserPromptSubmit` hook: slash commands, short answers, Claude Code's own turns, the task's own prompt, prompts nobody typed in the last minute, low confidence and Jev down pass untouched without blocking; a confident new scope is held with a reason for the person and nothing for the agent; "Send anyway" lets the identical prompt through once; parallel and "after" create tasks; drops scope marks the chosen steps `skipped` (counted as finished), messages the agent and chains the new direction; redirect is refused without confirmation and then keeps committed and uncommitted work on a `yulius/reverted/…` branch, resets the worktree, archives the task and starts the new one. The Jev adapter accepts only complete `jev-1.13.0` answers and makes exactly one request per check. | unit, int (fake Jev, real Git), ui (mock) |
| AC-TSK-9 | A goal-mode task types `/goal <prompt>` into the session once it is ready (fake agent), prompt mode passes the prompt as before; the mode round-trips through the API. A live Claude Code smoke shows the goal being set. | int, live |
| AC-TSK-13 | Fake agent shaped like Claude Code 2.1.292, running its hooks through the real `yulius hook` client: a goal task whose goal is recorded as achieved after a clean Stop is `done` with 0 background shells and stays running; a goal turn that leaves a Bash-tool background shell reports 1 shell (shown Working, "1 background shell still running") until the shell ends, then 0 (Done); typing after Done publishes `working`, and after that turn the task is `waiting`; after a server restart that replays the transcripts, a done task never becomes `working` and a goal achieved while the server was down (its Stop hook unanswered, the task left `working`) makes the task `done`. Mock UI: the goal-achieved task is in the sidebar's and Tasks' Done group with a green check and the header says Done; the one with a shell is under Working with the hint. | int, ui (mock) |
| AC-ROT-7 | The accounts API reports provider windows, observed tokens per account (24 h, 7 d) and unknown values as null; background rounds refresh usage and back off on HTTP 429 for `Retry-After` without losing the last values; an observed limit fills its window at 100 % with the reset. | int |
| AC-ROT-8 | With every Claude account exhausted the task waits for capacity (no harness change); refreshed usage at the threshold moves a running task to the next Claude account at its next turn boundary with a native resume; creating a Codex task or a Codex account is refused (422) and no harness setting exists. | int |
| AC-ROT-9 | Placement spends capacity that expires first (40 % weekly left resetting in 8 h beats 90 % left resetting in 6 days); concurrent tasks spread over subscriptions in proportion to their urgency; equal scores go to the capacity that expires soonest; known headroom beats a tight one beats unknown usage; a per-model cap binds only tasks of that model. | unit, int |
| AC-ROT-10 | With every subscription at 98 % or limited: a task that would work on is held at its Stop hook and a submitted prompt at `UserPromptSubmit` (`202` with `Retry-After` and no body, the prompt kept for a resume elsewhere); an agent without hooks is left idle; nothing is typed into the agent (its echoes stay what the user typed); the pause is persisted, a restarted server still holds the turn and reattaches the paused run, and the hook client keeps polling across a refused connection or a 5xx. | int, unit |
| AC-ROT-11 | When capacity returns, paused tasks resume oldest first one stagger apart: in place when their subscription's window reset (same run, nothing typed), on another subscription by native resume of the same session with no prompt, on a newly added subscription for tasks waiting to start; a turn the CLI interrupted with a limit continues with "Continue." only. | int |
| AC-ROT-12 | The forecast for a known history matches ADR-17 §2.4: burn rates (rises between readings, a reset counted from zero), the run-out time walking through each subscription's weekly reset, paused and active task-hours, needed subscriptions and the recommendation; `GET /api/capacity` reports them, ids and task counts for administrators only. | unit, int |
| AC-ROT-13 | The Accounts page shows the capacity panel and per subscription its windows, with only Add subscription, Sign in again (broken credentials) and Remove; no priority, order or on/off control exists in the UI or API (`PATCH` with `priority` or `enabled` → 422; views carry neither); the Dashboard shows the subscriptions card. | int, ui (mock) |
| AC-ROT-14 | One subscription alone: tasks start on it, pause at 98 % at their boundary, and continue after its reset. | unit, int |
| AC-BRW-7 | "Open live in a new window": a fixture app on a sandbox port opens on its preview origin through a single-use handshake (a grant cookie scoped to that origin, refused for other ports, sandboxes, read-only users and after sign-out); its root-relative stylesheet and image, a redirect to `localhost` and a WebSocket work; the app's `X-Frame-Options`, `frame-ancestors`, cookie `Domain` and `localhost` redirects and CORS origins are rewritten and it sees a plain `localhost` request without Yulius's cookies; cross-site callers cannot use a preview; a page on a preview origin cannot call the Yulius API or open its WebSockets with the user's session. | int (proxy) |
| AC-BRW-8 | No recursion: a preview of Yulius's own port or of an app that answers as this Yulius is refused; the sandbox Chrome refuses Yulius's addresses before navigating and fails a page's own request to them (the welcome page passes), and a browser without that guard is not used; the previews API and the welcome page skip Yulius's ports; Yulius's responses forbid every frame and the UI shows only a notice when framed. | int, live (Chrome) |
| AC-BRW-4 | The viewer socket of a browser on a virtual display sends `stream` video, the decoder configuration and frames starting at a keyframe; a second viewer starts at once from the cached group; the viewport resizes the display and restarts the encoder at the new size; a broken encoder or a browser without video decoding gets labelled JPEG frames; the display stops with the browser. | int (fake display) |
| AC-BRW-5 | A new tab opens the welcome page, which names the sandbox and links a listening sandbox port. | int, ui (mock) |
| AC-BRW-6 | An agent's click reaches viewers with its position and label, and the UI shows the labelled cursor over the streamed page; a page opening a page brings it to the front; watching a tab brings it to the front. | int, live (Chrome on Xvfb) |
| AC-BRW-9 | A viewer's click, drag and keys reach the display as real input (XTest), the wheel scrolls by CSS pixels, a Polish letter and short text are typed as real key presses in order, a long paste is inserted, ⌘/Ctrl+C answers with the page's selection, and held keys are released when the viewer leaves; on the VM the page sees trusted events, the typed text "Hello ą!", the scroll, and the change in the decoded video. | int (fake display), live (Chrome on Xvfb) |
| AC-BRW-10 | On the VM, in the headful Chrome through XTest: `navigator.webdriver` is false; the reCAPTCHA v2 demo checkbox shows a checkmark or an image challenge whose tiles are clickable; a Cloudflare Turnstile/managed challenge checkbox can be clicked. | live (manual record in the PR) |
| AC-BRW-11 | On Linux, inside the agents' and users' bubblewrap: no X socket, no cookie, no abstract socket and no TCP listener; `xdpyinfo -display :n` fails for every display; outside, a display answers only with its own cookie, not another sandbox's. | live (Linux) |
| AC-BRW-12 | Measured on the VM with real Chrome: frames per second while scrolling (p50, p5), input-to-frame delay (p50, p95), encoder CPU at 1280×800 and 1920×1080, bandwidth idle and scrolling; reported in the PR. | live (measurement) |
| AC-EDT-2 | The file API serves write users only, keeps every path (symbolic links included) inside the folder, hides `.git`, writes atomically and refuses a stale save with 409. | int |
| AC-EDT-3 | Opt-in browser test: the real UI in headless Chrome opens the editor panel, colors JavaScript, suggests `console` members, saves with Cmd/Ctrl+S, and suggests words in a Python file. | live (Chrome) |
| AC-EDT-4 | Word wrap is off in a new browser; the toolbar's Word wrap toggle turns it on (pressed) and Alt+Z (⌥Z, whose "Ω" is never typed) turns it off and on again in the editor, for the open file and the next one; after a reload the toggle is pressed and Monaco wraps; the shortcut sheet lists it. | ui, live (Chrome) |
| AC-PRJ-1 | Creating/importing a sandbox requires a repository of the instance owner; one repository maps to one sandbox. | int |
| AC-PRJ-2 | Task worktrees contain only the sandbox repository; there is no UI/API to add another repository. | int |
| AC-PRJ-3 | Creating from each template creates the GitHub repo (fake), pushes the scaffold with the docs format that passes the validator, configures protection, and opens the tab. | int, ui |
| AC-PRJ-4 | Removing a sandbox stops sessions and keeps the GitHub repository untouched. | int |
| AC-PRJ-5 | Two sandboxes receive non-overlapping port ranges; the briefing and environment expose them. | int |
| AC-UI-1 | Default view: sidebar + full-width terminal; browser/editor collapsed to icons; no top usage bar; Settings only behind the user menu. | ui |
| AC-UI-2 | Sandbox tab row ≤ 36 px lists accessible sandboxes and switches between them. | ui |
| AC-UI-3 | Sidebar icons reflect task status transitions within 2 s. | int, ui |
| AC-UI-4 | Task view has no Session/Changes/Commits/Files header; step strip and action menu work. | ui |
| AC-UI-5 | URLs printed in the terminal are clickable; a Yulius tab link opens the browser panel on that tab. | ui |
| AC-UI-6 | At 390×844 the task list, terminal and quick-input bar work (send prompt, Ctrl-C). | ui |
| AC-ARC-1 | Archive removes a task from the sidebar; Archived tasks page lists and restores it. | int, ui |
| AC-STA-1 | Status panel shows VM metrics and accounts usage; hidden by default. | ui |
| AC-UI-7 | Opening the right-rail Shell shows a working contained terminal for write users without a click (a new one in the sandbox repository when they have none); new terminals start in the selected folder; read users cannot open it; it is collapsed by default. | int, ui, live (Chrome) |
| AC-TSK-1 | Creating a task launches Claude Code in a new worktree on a new branch; task is one session lineage. | int (fake), live |
| AC-TSK-2 | Terminal shows the native TUI byte stream; typed slash commands reach the harness. | int (fake), live |
| AC-TSK-3 | When the agent process exits normally, the task becomes `closed`; no shell is ever spawned. | int |
| AC-TSK-4 | A session-id change inside the harness is detected and recorded into the same task lineage; no other task's transcript changes. | int (fixture) |
| AC-TSK-5 | Two tasks in one sandbox have distinct worktrees and branches; cleanup never removes a dirty or foreign worktree. | int |
| AC-TSK-6 | Two viewers see identical output; input from a read-only viewer is rejected. A viewer alone keeps the size while idle; a viewer who opens the task takes it from an idle one; an active viewer does not take it from another active one until it types or clicks (`take_control`), which applies its window's size at once; a watcher's reported size waits; an idle or leaving controller hands the size to the active viewer (the agent's `stty size` shows it). | int, unit |
| AC-TSK-7 | A closed task continues via resume of the same provider session id. | int (fake), live |
| AC-ROT-1 | Admin adds Claude accounts via the login terminal; accounts list shows status and usage. | int (fake CLI), ui, live (manual) |
| AC-ROT-2 | Usage refreshed at 98 % (either window) triggers a switch to the best other Claude account at the turn boundary, never mid-turn; the session resumes with the same session id; decision recorded. After a reported limit the account is `limited` with its window at 100 % until the reset, and a new task starts on another account. | int |
| AC-ROT-3 | With all accounts exhausted and one window reset passed, that account is chosen. | unit, int |
| AC-ROT-4 | Withdrawn with ROT-005/006 (F 2026-10-08). | — |
| AC-ROT-5 | Manual switch to a chosen account preserves model/effort. | int |
| AC-ROT-6 | No capacity anywhere → the task is paused (`waiting_capacity`) with the earliest reset (`resumes_at`); resumes when a reset passes. The limit is recognised from Claude Code 2.1.292's structured limit record ("You've hit your session limit") as well as older texts. | int (fake clock), unit (transcript records) |
| AC-STP-1 | Agent sets/updates steps via Yulius tools; history records actor and time; UI shows current step. | int, ui |
| AC-STP-2 | Step strip shows "n/m current step" and expands to the list. | ui |
| AC-STP-3 | Editing the spec Plan checklist updates steps; step updates are reflected in briefing. | int |
| AC-COM-1 | `tasks_list` returns previous and parallel tasks with status, branch, spec and summary. | int |
| AC-COM-2 | A task messages a parallel task; the recipient receives it at its next turn or is woken when idle. An answer to people is in the sender's thread and published to the UI, and no task receives it. The Messages panel has no text box or Send button; an agent's question to people offers "Reply in the terminal", which focuses the task's terminal; the rail keeps the unread count; `POST /api/tasks/{tid}/messages` answers 405. | int (fake), live, ui (mock) |
| AC-COM-3 | Overlapping scope claims are reported to both tasks. | int |
| AC-COM-4 | Session start receives a bounded briefing with project summary, decisions, recent changes and parallel tasks. | int |
| AC-DSC-1 | Validator accepts the scaffolded format and reports precise missing items otherwise. | unit |
| AC-DSC-2 | Each task has one spec path registered; delivery refuses without it. | int |
| AC-DSC-3 | Turn end with code changes but invalid spec blocks the agent with a reason listing missing sections, at most 3 consecutive times, then marks needs attention. | int |
| AC-DSC-4 | Briefing/skills instruct technology decision + ADR; a new project's first task produces an ADR. | int (briefing content), live (manual) |
| AC-DSC-5 | Pushes from task worktrees to integration/production branches are rejected locally; scaffolded projects get GitHub protection. | int |
| AC-DLV-1 | Standard profile: PR into dev, merge after green checks, promotion PR dev→main. | int (fake GitHub) |
| AC-DLV-2 | Product profile: PR into quartio, dev-VM deploy step requires explicit click and records output, then ff main, then rollout step. | int (fake commands) |
| AC-DLV-3 | Studio profile: after merge into quartio, main fast-forwards automatically; changing profile to gated disables auto-promotion. | int |
| AC-SKL-1 | Repository and instance skills are installed into the Claude home so they activate by description. | int, live |
| AC-SKL-2 | The OM pack contains exactly the selected skills with license/attribution. | unit |
| AC-SKL-3 | Skills page lists sources and toggles. | ui |
| AC-MCP-1 | A configured MCP server appears in the Claude launch configuration (`--mcp-config`). | int, live |
| AC-MCP-2 | MCP secrets are encrypted at rest and never returned by the API. | int |
| AC-AUT-1 | A cron automation creates a task at the scheduled time (fake clock). | int |
| AC-AUT-2 | An agent creates an automation through the Yulius MCP tool; it appears in the UI. | int |
| AC-AUT-3 | Creating a task from a workflow seeds its steps and instructions. | int |
| AC-AUT-4 | Restarting during a scheduled run never creates a second task for the same slot. | int |
| AC-GIT-1 | Git page shows task branches, diff stats, PRs and checks (fake GitHub). A ready task opens its PR by itself (not with an open step or uncommitted work, once per task), and a merge on GitHub moves its stage in the background. With the fake GitHub answering 403 on check runs and 200 on statuses, the checks come from statuses and Actions runs; with 403 on statuses, from check runs; with nothing readable they are unknown, a merge or release that requires checks is refused with the reason, and the missing permission is in `checks_access` once (the settings link for instance administrators only), with no note repeating "GitHub:". | int, ui |
| AC-GIT-2 | No API path merges or promotes from agent output without a user action or explicit policy. | int |
| AC-BRW-1 | Per-sandbox browser: typing a URL navigates; another sandbox has a separate profile. | int (fake CDP), ui (real Chromium, manual) |
| AC-BRW-2 | Agent browser tools navigate/click/type/read the same tab the user sees. | int (fake CDP) |
| AC-BRW-3 | `browser_request_user_action` highlights the tab, notifies the user and produces a clickable link. | int, ui |
| AC-EDT-1 | The editor panel lists, opens and saves files of the sandbox repository or a task worktree; read-only users cannot open it. | int, live (Chrome) |
| AC-IMP-1 | `yulius import` uploads bundle + snapshot resumably; interrupted upload resumes. | int |
| AC-IMP-2 | Import of a non-GitHub or foreign-owner repository is refused with guidance. | int |
| AC-IMP-3 | Only sessions whose cwd/repository match, within the chosen window, are offered. | unit (fixtures) |
| AC-IMP-4 | After import, the preparation task is created and other tasks are blocked until it lands or is skipped by an admin. | int |
| AC-IMP-5 | Another member continues an imported session natively. | int (fake), live |
| AC-IMP-6 | Re-running import adds only new sessions. | int |
| AC-ANL-1 | Task metrics include durations, models, tokens per attempt, accounts and switches; unknown usage is null, not zero. A live run has metrics after its first turn (one row per run, updated in place). | int |
| AC-ANL-2 | Feedback yes/no + comment is stored per user and shown. | int, ui |
| AC-ANL-3 | Dashboard aggregates and CSV/JSON export work. | int, ui |
| AC-OPS-1 | Packaging: single binary + systemd unit + reverse-proxy example + install guide. Deployed behavior: **not verified (out of scope)**. | review |
| AC-OPS-2 | All UI traffic goes through a documented HTTP/WS API usable by another frontend with token auth and an origin allowlist. | int |
| AC-OPS-3 | `yulius backup` produces a consistent copy of the data directory. | int |
| AC-NFR-1 | Closing all browser connections leaves sessions running; reconnecting shows the current screen. | int |
| AC-NFR-2 | Killing and restarting the server reattaches live sessions; dead sessions are marked stopped without respawn. | int |
| AC-NFR-3 | Exceeding the concurrency limit queues tasks; queue bound rejects with a clear error. | int |
| AC-NFR-4 | A slow/lagging viewer is disconnected without blocking output; reconnect resynchronizes; input is applied once in order. | int |
| AC-NFR-5 | Cancel of a fake agent that ignores SIGTERM and forks a descendant ends with all processes reaped and one terminal outcome. | int |
| AC-NFR-6 | (Linux) A sandboxed task cannot read another sandbox's directory. On non-Linux dev hosts the UI states "not contained". | int (Linux CI) |
| AC-NFR-7 | Credentials never appear in argv, logs, API responses or the repository (redaction tests). | int |

## 11. Implementation evidence

Pending. Evidence is recorded per checkpoint in GitHub issues and in SPEC-002 §14 as it is
observed. Live provider compatibility and deployment are reported separately from synthetic tests.

## 12. Changelog

- 2026-10-10 (`F 2026-10-10`, item 5): each viewer sees the terminal full size — the active viewer
  sets the size automatically, the others see that grid scaled to fill their box with a small chip
  (no "Take control"); Ctrl+C/Ctrl+V copy and paste as in desktop terminals with a Copy/Paste menu;
  file paths in terminal output open the editor at the file and line (UI-018, UI-019, TSK-006
  changed; AC-UI-10, AC-UI-11, AC-TSK-6 changed).
- 2026-10-10 (`F 2026-10-10`): a task whose goal Claude Code records as achieved is Done once its
  turn ended cleanly and no background shell is left; one written rule for Done (goal achieved or
  tested pull request), the session stays open and typing makes it Working again; a restart no
  longer turns finished tasks back to Working (TSK-014 new, AC-TSK-13; GIT-001, UI-043 and §7.1
  refer to it).
- 2026-10-10 (`F 2026-10-10`, item 10b): Claude subscriptions are one self-managing pool —
  use-it-or-lose-it placement without priorities, a 98 % switch at the turn boundary, a gentle
  pause held by the harness and invisible to the agent, staggered resume, and a capacity forecast
  with a recommendation; the Accounts page is add / usage / remove (ROT-003, ROT-004, ROT-009,
  ROT-010, ROT-012 changed, ROT-013 new; §6.2, §7.1, §7.4; AC-ROT-2, AC-ROT-6 changed,
  AC-ROT-9..14 new).
- 2026-10-10 (`F 2026-10-10`, items 2 and 8): the Messages panel shows only messages between
  sessions and tasks and agents' messages to people, with "Reply in the terminal" for an agent's
  question; the user → agent composer and `POST /api/tasks/{tid}/messages` are gone (COM-005,
  AC-COM-2; #110). Checks fall back to the sources the GitHub App may read; a missing permission
  is one dismissible line for instance administrators; unreadable checks are unknown and never
  count as passed (GIT-001, AC-GIT-1; #111).
- 2026-10-10 (`F 2026-10-10`, item 10 c/d): task intake with Jev — a gentle split offer for
  requests that mix separate workstreams (smallest split, grouped by area, never one task per
  item), scope discipline for prompts typed into a running task (held new scope, dropped scope and
  redirect with their choices, "Send anyway"), skipped steps, and chained tasks that start when
  their predecessor is Done (TSK-011..013, HRN-006, STP-005; AC-TSK-10..12).
- 2026-10-10 (`F 2026-10-10`, layout): the sidebar, the editor's explorer and the tool panel resize by
  their edge, kept per user; `/` is a home screen with every sandbox; headers, toolbars and lists wrap
  instead of overlapping; pages are centred without an empty band (UI-044..UI-047, AC-UI-39..42).
- 2026-10-10 (`F 2026-10-10`): the browser panel streams the sandbox Chrome itself — headful, on
  its own virtual display, as low-latency video with real input — so scrolling and typing are
  fast and captchas work; the DOM mirror and the panel's live tabs are gone, a dev server can still
  open live in its own window (BRW-005..008 changed, BRW-009/010 new; AC-BRW-4..7 changed,
  AC-BRW-9..12 new).
- 2026-10-09 (`F 2026-10-09`): the browser panel renders the sandbox's apps live in the user's
  browser through per-port preview origins; the sandbox Chrome stays for agents, watched remotely
  with the cursor; no recursion (BRW-005, BRW-006, BRW-007 changed, BRW-008 new; AC-BRW-4..6
  changed, AC-BRW-7/8 new).
- 2026-10-09 (`F 2026-10-09`, evening): terminals are the user's, many per sandbox as tabs, opened at
  once without a button, private per user and sandbox, and kept across server restarts (UI-015,
  UI-016, AC-UI-7, AC-UI-8, AC-UI-37); the task terminal appears once at its final size and font,
  with the session's state in the strip, empty states and floating notices (UI-026, UI-043,
  AC-UI-38).
- 2026-10-09 (`F 2026-10-09`): the editor's word wrap is a toolbar toggle and ⌥Z / Alt+Z, kept per user in the browser (EDT-005, AC-EDT-4).
- 2026-10-09 (`F 2026-10-09`): the tools rail keeps its items, counts and focus ring inside it with one clear open tool, and an open panel no longer pushes it out of a narrowing window (UI-042, AC-UI-36).
- 2026-10-09 (`F 2026-10-09`): the superadmin owns the instance and sees and opens every sandbox
  with admin access without a GitHub link; GitHub actions go through the App where Yulius already
  acts as it, merging and releasing tell the superadmin to use GitHub (ACC-012, AC-ACC-8; U-06
  superseded; ACC-007 and ACC-009 note the exception).
- 2026-10-09 (hardening): open connections follow revoked access and ended sessions (ACC-008).
- 2026-10-09 (hardening): the five Yulius tools the briefing announced but the MCP did not serve now exist: `scope_claim`, `scope_release`, `scopes_list` (COM-003, AC-COM-3), `spec_check` (DSC-003) and `project_context` (COM-004).
- 2026-10-09 (hardening): browser and editor tabs work from the keyboard and with screen readers (no button nested in a tab; ←/→, Delete closes; listed in the shortcut sheet, UI-029); the task preview's sessions table scrolls from the keyboard.
- 2026-10-09 (hardening): agents can answer people (`message_send` `to="people"`) (COM-005, AC-COM-2).
- 2026-10-09 (`F 2026-10-09`): a terminal sized by another viewer is fitted to this view's width instead of
  leaving an empty band (UI-019, AC-UI-11).
- 2026-10-09 (`F 2026-10-09`): the server serves the task review's changes and commits, so Git → a task
  shows Changes and Commits (UI-031).
- 2026-10-09 (`F 2026-10-09`): a task ready for delivery opens its pull request by itself, stages follow
  GitHub in the background, and a task with a tested PR is Done (In review while checks run)
  (GIT-001, AC-GIT-1, UI-024).
- 2026-10-09 (`F 2026-10-09`): every Tasks row shows its author again, yours included (UI-024, UI-032).
- 2026-10-09 (`F 2026-10-09`): the task terminal scrolls with the wheel again: a full-screen program
  that asked for the mouse gets the wheel turns while clicks keep selecting text (UI-019, AC-UI-11).
- 2026-10-09 (`F 2026-10-09`): run metrics are stored live (after each turn, at least every 30 s)
  with active agent time, and an open Dashboard refreshes itself (ANL-001, AC-ANL-1).
- 2026-10-09 (`F 2026-10-09`): accounts know their limits from what the CLI reports (the limited
  window at 100 % until its reset), new tasks skip limited accounts, usage refreshes in the
  background and a throttled usage request backs off for `Retry-After` (ROT-003, ROT-009,
  AC-ROT-2, AC-ROT-7).
- 2026-10-09 (`F 2026-10-09`): usage limits are recognised from Claude Code's structured limit record,
  so a paused task no longer shows "Working"; waiting for capacity shows as "Usage limit reached"
  with the reset time under Needs you (ROT-010, AC-ROT-6).
- 2026-10-09 (`F 2026-10-09`): a refused GitHub sign-in returns to the sign-in page, which names
  the refused GitHub account and offers GitHub's account picker; no raw error, and a reload
  shows the sign-in page (ACC-004, AC-ACC-3).

- 2026-10-07 (feedback on the deployed instance, `F 2026-10-07`): recovery code (ACC-002
  changed, ACC-011), user-scoped shells with per-user root identity (UI-016, U-12), clean-exit UI
  (HRN-002), task deletion (TSK-009), goal mode (TSK-010), harness changes need approval (ROT-004
  changed, ROT-012), account usage (ROT-009), DOM-mirrored browser and welcome page (BRW-005/006),
  a built-in VS Code-styled editor replacing openvscode-server (EDT-001 changed, EDT-003/004);
  later the same day: the agent's cursor (BRW-007), the hidden tools rail (UI-017), link routing
  (UI-018), HTML terminal text with instant copying and double-click selection (UI-019), text
  selection in the browser mirror (BRW-005) and the "unknown tab" error (BRW-004).

- 2026-10-08 (UI audit wave 3, `F 2026-10-08 UI`): automation schedules from presets with cron under
  Advanced (UI-027, AC-UI-20); notification filter, per-row read and opt-in desktop notifications
  (UI-028, AC-UI-21); a keyboard shortcut sheet (UI-029, AC-UI-22);
  the pull request and next delivery step in the task view (UI-030, AC-UI-23); the Git page's task
  review (UI-031, AC-UI-24; the diff and commits need the proposed task changes API); the Tasks
  table and a task's overview (UI-032, AC-UI-25); the skill editor (UI-033, AC-UI-26; saves repository skills through the files API); the task terminal's status line (HRN-005, AC-UI-27; proposed for the harness); the workflow builder (UI-034, AC-UI-28); New skill (UI-035, AC-UI-29; files API); the Spotlight-style task search (UI-036, AC-UI-30); filter tabs and styled lists (UI-037, AC-UI-31); the MCP server page (UI-038, AC-UI-32); the automation builder (UI-039, AC-UI-33); the dashboard layout (UI-040, AC-UI-34); the Roma brand direction and its smoke (UI-041, AC-UI-35).

- 2026-10-08 (UI review, `F 2026-10-08 UI`): the user menu moves to the sidebar's foot and the
  collapsed sidebar is dropped; the tools rail names its tools; the sidebar shows the Yulius
  wordmark; a notification center behind a bell (UI-020, AC-UI-12); the composer becomes a rounded
  prompt box with mode, workflow, model and effort pickers (UI-021, AC-UI-13); the Status panel opens
  with a health line and machine tiles, and accounts read at a glance (UI-022, AC-UI-14; email and
  plan still need the server); a Settings page behind the user menu and icons in it (ACC-010 changed,
  AC-UI-15).

- 2026-10-08 (`F 2026-10-08`): Yulius runs Claude Code only. Codex support, provider switching
  and session translation are pruned and deferred until Codex improves (TSK-001/003, ROT-001/004/
  007/012 and IMP-003/004 changed; ROT-005/006 and AC-ROT-4 withdrawn; COM-002, SKL-004, MCP-001,
  BRW-003/007 reworded). Account rotation between Claude accounts stays automatic.

- 2026-10-07: initial specification from the full transcript, video review, morning notes, draft
  prompt, user clarifications and Product/Studio inspection; self-reviewed (SPEC-003).
