# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-13

- `/compact` typed into a session leaves its task as it was (SPEC-085 STT-005, ADR-0068): Claude
  Code's line for the command no longer reads as a prompt, so an idle Done task no longer reads
  Working after the compaction; a prompt typed after it is work as always. While Yulius itself
  recovers a full session the task reads Working, not Your turn, until it types the prompt that
  carries on (STT-006, CTX-002 changed).
- A task's status tells the truth: Working is work, Needs you is a person's move (SPEC-085,
  ADR-0068; F 2026-10-13 "marked as running even if was done", "needs you was too often"). A
  replay of two days of transcripts found "Needs you" false for 43 of its 50 task-hours:
  - An agent waiting in `yulius wait` for its checks or armed merge reads "Waiting for checks or
    merge" under Working, not Your turn (173 periods, 37 tasks). The process probe counts such a
    shell apart; disk upkeep, DLV-203 and Done still leave it out (DSK-102).
  - Failing checks need you only once the agent stopped: while its background agent or shell
    runs, or it waits for Yulius, the task reads "Working · PR #N checks failing" (task #63).
  - Records Claude Code writes for itself start no turn: an idle compaction's summary made Done
    task #60 read Working until it was stopped; so did command echoes and their output.
- Yulius's tools answer in a few readable lines, and sessions fill the page (SPEC-086, ADR-0069):
  - A tool's result was its whole record as pretty JSON, which filled the agent's terminal (task
    #66's steps list) and its context: 6.08 MB in the transcripts of 2026-10-12/13, most of it
    `ci_status`, `step_update` and `delivery_status`. Each tool now says what changed and what is
    next, with the numbers and ids the agent passes back: the plan as a checklist, a step update
    as the changed step, the progress and the next step, CI as a line per run with the output of
    failed jobs only (`job` asks for another), delivery as the stage, the pull request and what
    blocks it. Replayed over those results, 71 % less text. A client that reads records asks
    with `_meta: {"yulius/structured": true}`; Claude Code would show the model a
    `structuredContent` instead of the text, so it is never sent by default.
  - Agents read files with `cat`, `sed -n` and `grep` because Claude Code tells them to in
    bypass mode. Measured, it costs nothing (0.3 % rereads, smaller reads than `Read`, no failed
    edit), so it stays.
  - A session's transcript fills the content area edge to edge, without side margins, border or
    rounded corners, beside the Superagent too and on phones; the header keeps its padding.

- The original prompt's thumbnails always load (SPEC-080, ADR-0064, task #63; #303). Reading a
  prompt's attached file no longer takes an upload slot: reads have their own bound of 16 and wait
  up to 30 s for a slot, so a prompt with many images, or one opened while uploads run, shows every
  thumbnail. The file is streamed in 64 KiB chunks under its slot; a stalled reader gives the
  slot back and its file is closed after 60 s. On Linux a file whose `/proc/self/fd` link cannot be read is refused. A prompt
  starting with `---` shows as text, not as properties, and the mock serves only the task's own
  attachment folders.
- An idle task's background work and its goal verdict decide its status (SPEC-079, ADR-0063, task
  #60; #305, #264, #265, #271):
  - While a goal is open, a clean Stop leaves the task Working until Claude Code's goal verdict,
    bounded by the turn record or 20 s. "Not met" no longer flashes Your turn or Done, and no chain
    starts from a Done the verdict takes back.
  - Monitor streams count as background work until their stream ends or expires. Claude Code's
    turn record does not count them, and a WebSocket monitor has no process.
  - A proactive switch at a Stop waits while a background agent the session launched is still
    pending, even one launched in the turn that just ended. The switch used to end the agent.
  - When a Done task's last background shell or agent ends, the queue drains at once instead of at
    the next tick. So does a Stop that makes a task Done. Claude Code's wake-up prompt no longer
    takes back an achieved goal, so the task woken by its finished work ends Done again.
- An automation starts its tasks like New task (SPEC-078, ADR-0062, task #70; SPEC-001 AUT-013).
  The builder's **Start** row, the API (`supertask`, `after_task_id`, `start_rule`) and the tools
  `automation_create` / `automation_update` pick one of four choices: Start now (queued, the
  default), Supertask, After a task, or When a rule is true. Each run creates its task with that
  choice. After a task that has ended, the task waits for the rule `#N done` instead of failing.
  The sentence and the list row name the choice, and a waiting task keeps the next runs skipped.
  Schema version 19.
- Home's cards and the sandbox tabs count "working" and "queued" by the sidebar's rule (SPEC-076,
  ADR-0061, task #62; #170). One domain rule (`AttentionFacts::group`) gives each task its sidebar
  group: "queued" counts queued and chained tasks, "working" the rest of the Working group
  (running, starting, continuing, paused, in review), so a card's working + queued is the
  sidebar's Working count. A Done task or one being stopped counts in neither, and a task being
  stopped no longer counts as needing you.

- Resuming after a usage limit is safe in three more cases (SPEC-072, ADR-0058, task #57; #276,
  #277, #278):
  - Claude Code sometimes continues a turn by itself at the reset. Its first agent work or turn end
    now ends the task's limit pause, so Yulius no longer terminates a turn the CLI is running. After
    a restart, a limit the CLI already continued past is not acted on again.
  - When the automatic switch after a limit is refused, the run remembers it is at its limit. A
    person's switch then resumes past the limit, without "Continue the task exactly where you
    left off.".
  - The transcript step back runs on a blocking thread, off the async runtime. The session's
    search index is rebuilt only if the step back changed the file.

  New tests cover a resume past a limit that finds no capacity, a limit pause whose process is
  gone, and goal mode after a continued turn.
- A task is owned by one person, whose GitHub credentials it uses (SPEC-070, ADR-0060, task #66).
  Every task has an owner, its creator until a hand-over. The task's Git, `gh`, pull requests,
  merges, commit identity and agent authority are the owner's. Only the owner changes the task:
  anyone else, administrators included, gets 403 naming the owner, watches the terminal
  read-only and cannot write into the task's worktree or open a shell there. They can ask for control (Request control in the task). The owner hands it over or
  declines, and the asker may withdraw. From the hand-over on, the task acts on GitHub as the new
  owner, and its terminal says so. An owner who is disabled, deleted or without write access
  passes control at once. Schema version 18 (`tasks.owner_id`, the waiting request).
- A session opened from Sessions reads as Claude Code's TUI printed it (SPEC-075, ADR-0059, task #71).
  Prompts and slash commands show as `>` lines and messages and tool calls as `⏺`, with each result
  under `⎿` folded to its first lines ("Read 2 lines", "… +2 lines"). Edits show as `Update(path)`
  with their lines. The raw `<command-name>` and `<local-command-stdout>` tags are gone, and
  reminders are hidden. The transcript is faded in both themes: fainter and less saturated than a
  live terminal (`--session-*`), yet still WCAG AA. It ends with a locked prompt box, and a
  pixel-art spider web hangs in its corner.
- The Agent panel is now called the **Superagent**, and a usage limit no longer stops it (SPEC-074,
  ADR-0055, task #69). The Superagent chose a subscription once, when it started, and kept it, so
  it could hit a weekly limit while tasks went on elsewhere. It now watches its conversation. A
  limit recorded after it started is recorded on its subscription, and the Superagent starts again
  on one with capacity. It continues the cut turn there, stepped back over the limit as a task's
  turn is, and the panel follows it without a "stopped" banner. Without capacity, it stays and
  says why. `/s/<slug>/superagent` opens it, as `/agent` and `/cockpit` still do.
- Supertasks (SPEC-071, ADR-0054, task #67; SPEC-001 TSK-018). Choose **Supertask** in the
  composer's start menu (API `supertask: true`, `task_create`) for broad work such as security
  hardening. The task waits for its project's running tasks, then runs alone. While it waits or
  runs, no other task of the project starts, also tasks queued or chained before it; they wait
  `queued` and say "Queued after supertask #N". A queued supertask says "Supertask · waits for
  #3 and #5". Running sessions are never interrupted, and other projects keep running. At the
  session limit, a supertask gets the next free slot first. Schema version 17
  (`tasks.supertask`).
- Cockpit and terminal starts survive a dropped request (SPEC-077, ADR-0053, task #64; #250). The
  part of a start from the spawn to the registration runs as a task the request awaits, so a
  client that goes away mid-start leaves one live Cockpit or terminal (or nothing on an error),
  never a second Cockpit, an unusable terminal or an orphan run directory. Idle Cockpit start
  locks are dropped. New tests pin that a signal right after a resume keeps the conversation and
  that one user's blocked start delays no other user or sandbox.
- A task's step strip no longer draws its progress bar over "Step N/M" (SPEC-073, task #68; #341).
  A long state such as "Working · 1 background task still running" or "Done · PR #N merged" kept
  its full width, so the strip's label shrank to nothing and the step count, which nothing
  clipped, ran under the bar. The same happened with a short "Done · PR #N" in a narrow strip
  beside a tool panel. The label now keeps room for the step count, the state gives its words up
  first (it takes at most 40% of the strip, and its full text stays in the tooltip), and the label
  is clipped inside its button.
- A CI run that could not run (disk, a timeout, the host) reads "CI could not run (not the code)"
  instead of "checks failing · Needs you"; an armed merge stays armed and the agent is told to run
  the checks again (SPEC-050 STA-101).
- Local test runs use 4 test threads (CI keeps one per CPU): several agents running the suite
  at once in one sandbox starved their tests into timeouts.
- Only real typing counts as typing; a typed first message is never lost (SPEC-068, ADR-0052,
  task #61, #252, #266). Focus and mouse reports and terminal answers no longer record typing or
  take control of the terminal size. A goal task whose terminal a person typed into before its
  goal leaves the draft alone and needs attention, instead of appending `/goal` to it. A first
  message with images that a server restart cut flags its task with a notice; one held for
  capacity and resumed (or manually switched) to another account is typed again there, images
  included.
- Yulius's merge gate enforces the branch policy (SPEC-069, ADR 0051, task #59; #223). Before
  Yulius merges a task's pull request (`delivery_merge`, an armed merge, the Git page) or a
  promotion pull request, it checks the title, head and base against `yulius.toml`
  `[branch_policy]` at `dev`. The standard preset is the default: typed branches, conventional
  one-line titles of at most 100 characters, and `dev` → `main` titled
  `release: promote dev to main[ - label]`; `preset = "none"` turns it off. A violation is
  refused with the rule in words, and `delivery_merge` refuses before it arms. Yulius's own
  titles comply: task titles take an allowed type and are clipped to 100 characters, promotion
  pull requests are titled `release: promote dev to main`, and a fork's pull request is never
  taken for the promotion. `scripts/check_git_policy.py` is removed.
- A stopped or closed run ends only when its processes are proven gone (SPEC-065, ADR-0050, task #54;
  #284, #285, #290). When Yulius cannot terminate a run's host after the stop or close escalation,
  the run stays live, the task's terminal says so and a running task needs attention; deleting it
  answers 409 and removes nothing. Deleting a Done task now returns only after the run's end is
  fully recorded, so a task chained after it gets one outcome. The wait re-checks every second even
  on a busy instance, a stuck host can no longer cancel the escalation, and a task that turned
  switching is refused at once. The web UI sends one delete per task, shows "Deleting #N…", and a
  stopping task's Delete says "Stopping…".
- Five flaky tests wait on what they check (SPEC-067, task #56; #243, #320, #322, #294, #269).
  Each was reproduced next to CPU-bound processes, and none got a sleep or a retry. Two were
  product bugs:
  - A task's header strip now follows its pull request read. A task whose pull request is tested
    reads "Done · PR #N" even when that read lands after the view opened. Before, it kept "Your
    turn" until an unrelated event redrew it.
  - A SessionStart hook answered before Yulius watched the new run was forgotten. The task's goal
    then waited 45 s before it was typed. The flag is now reset before the agent is spawned.

  The slash-menu, Auto, stored-draft and terminal-link tests now wait for the composer's focus,
  hold the creation, store the draft before the page opens, and wait until xterm knows the link
  and the Editor has loaded. The dropped-goal test sends Claude Code's SessionStart hook: its goal
  starts after 16 s instead of 60 s.
- The Cockpit and task agents manage the whole task and automation lifecycle (SPEC-066,
  ADR-0049, task #58, #275, #293). New MCP tools `task_stop`, `task_archive`, `task_delete`,
  `automation_delete`, `automation_run_now` and `automation_template_enable` do what the UI
  does, for the person the caller acts for, audited with the caller (`task:<n>` /
  `cockpit:<login>`); a task never stops, archives or deletes itself. `automations_list` lists
  the ready-made automations (`ready_made`). The Cockpit is served them all and asks the person
  before each. Workflow create and edit stay UI-only.
- `yulius wait` on armed merges (SPEC-064, ADR-0048, task #53; #248, #249, #255, #251). A merge
  armed while Yulius still decides an earlier one supersedes it: the wait on the new head no
  longer ends with the old head's "dropped", and the new merge stays armed and saved. The armed
  merge, its decisions in progress and its outcome are read and changed under one lock, so a wait
  never misses a decision. A wait with a merge armed and checks pending keeps waiting (#249 did
  not reproduce on `dev`; a regression test now covers it). The automatic retry of an errored
  Yulius CI run at a pull request's head is tested: once, never a third time, never when the pull
  request conflicts.
- CLI device sign-in can no longer fill the disk (SPEC-063, ADR-0047, task #55, #169).
  `POST /api/device/start` is limited per client like password sign-in (5 starts a minute); at
  most 10 device sign-ins wait per client and 1000 per instance (past either: 429, nothing
  evicted); expired and consumed grants are deleted on every start and every minute. Schema
  version 16 (`device_grants.client_key`).
- A session whose context is full recovers (SPEC-060, ADR-0046, task #51, #213). Claude Code's
  "Prompt is too long" ends the turn, so the task waits instead of staying `working`. Yulius then
  types `/compact` with a focus built from the task (goal, branch, pull request, steps left) and
  wakes the compacted session. If the compaction fails, the task continues in a fresh session
  opened with a handoff. If the fresh session is full too, the task needs attention and says why.
  Each step shows in the task's terminal and the audit log (`task.context_full`).
- Unique decision numbers and the onboarding CI proof (SPEC-058, task #49; #317, #272). The four
  decision records numbered 0034 now have their own numbers: the automations catalog keeps 0034, the
  Agent in the right panel is 0036, the personal two-factor vault 0042 and the task status from
  Claude Code's records 0044 (after #333 took 0041); the archive-all record (#332), a later second
  0040, is 0043. `scripts/check_repository.py` fails on two records with one number, a title whose
  number differs from the file name, or a record missing from `docs/decisions/README.md` (0016 was).
  The onboarding prompt and infra-bootstrap step 2 add `YULIUS_CI_MATRIX` to the proof of matrix
  jobs and say how to clean up after an interrupted proof: the proof prints its directory, the agent
  removes its own by that path and other tasks' `~/.ci-proof.*` only when over six hours old, then
  runs `git worktree prune`. They also say that checkouts are per CI lane while by default up to two
  runs share the CI home and cache, and that system packages are installed by onboarding, never by a
  CI job.
- CI waits for disk instead of erroring (SPEC-059, ADR-0045, task #50; #240, #256, #258, #245). A
  CI run in a sandbox over its disk quota stays queued, with a `pending` status. It says who holds
  the space, and starts by itself once Yulius has pruned the caches of tasks that do not work, or
  other tasks have freed space. An armed merge keeps waiting for it. Quota refusals of sessions
  name the largest consumers too. Upkeep now probes a task's background shells again before it
  prunes. A running background build or subagent, or a probe that cannot answer, keeps the
  task's caches.
- Two-factor fixes (SPEC-061, ADR-0041, task #52, #291, #295). A lost authenticator no longer
  locks someone out of the secrets for good: an instance administrator or the superadmin resets
  their two-factor from the Users page (a confirmation, then their own code), and the person sets
  it up again on their next reveal. The reset is in the secrets activity (`vault.2fa.reset`).
  Instance administrators now see the Users page (read-only, with each person's two-factor). A
  code now confirms only the seed it was checked against, so a new key set up in another tab
  while a code is checked is not confirmed by it. API: `DELETE /api/users/{id}/two-factor`;
  `AdminUser` gains `two_factor`.
- Open GitHub issues triaged, and the web UI keeps the person's keyboard, scroll and text (SPEC-056,
  task #47). Issues already fixed on `dev` were closed with the change that fixed them, and the
  other actionable ones went to chained tasks #49–#64. In the web UI:
  - a terminal that shows late leaves the keyboard in the Editor on Chrome (#325), and with a
    terminal asked for after it (#323);
  - Notes shown again for a new page does not take the keyboard (#310);
  - a menu placed again keeps its scroll (#288);
  - the task view ignores an older detail read (#316);
  - the queued messages card keeps unsent text and a refusal when the task starts (#324);
  - leaving the composer revokes every object URL it made (#259).
- Quick archive (SPEC-062, ADR-0043, task #65). A sidebar task row that can be archived shows an
  Archive button beside its `⋮` on hover or focus, and the Done group's head has **Archive all**:
  one click archives every Done task, with one Undo for the whole batch. When some fail, the
  others stay archived and one toast says how many failed and why. Phones keep the `⋮` menu as
  their only row action. No API change: each task goes through `POST /api/tasks/{tid}/archive`.
- Done tasks no longer hold their subscription (SPEC-057, ADR-0040, task #48). A subscription
  card counts only tasks still working on it, so one whose tasks are all Done (goal achieved, or
  plan complete and merged: the session limit's rule) says "No tasks right now" and can be removed. Removing it closes those Done tasks' idle sessions first, as archiving
  does. The pool's load and the Status panel count the same way as the session limit (SES-101).
- CI jobs get the CPU first (SPEC-053, ADR-0039, task #45, #283). A sandbox's cgroup is split
  into `system/`, `agents/` and `ci/`, and CI jobs run with four times the agents' CPU weight. CI
  gets up to 80 % of the sandbox while agents build, and agents keep at least 20 %. The e2e suite in CI is
  back to the default budgets and no retries, so a timeout there is a real failure. The split is checked on the VM after the next update (#321).
- Onboarding follow-up (SPEC-055, SPEC-051, task #46): `docs/api.md` no longer says an import starts the
  preparation task (a `pending` sandbox waits for Start onboarding), ADR-0038's wording and the
  decisions index order are corrected, and the import test asserts the pending onboarding summary.
- Queue messages into a task that has not started (SPEC-052, ADR-0035, task #44). A task waiting
  for a session slot, after another task or a rule, or paused before its first start shows a
  "Queued messages" card: the request as the agent will get it, and a box to queue a correction
  (Queue, ⌘/Ctrl+Enter). Each message is added to the request under "Added by … before the task
  started (… UTC)", so the request file, the goal, the first message, the scope check and
  `task_get` all carry it. Once the task has started, its session is the way in (409). API:
  `POST /api/tasks/{tid}/queued-messages`; `TaskDetail` gains `request` and `started`.
- `yulius onboard` only uploads (SPEC-051, ADR-0038, task #43): the repository, branches, snapshot
  and sessions land, and the onboarding task that adapts the repository to the Yulius standards
  waits until someone clicks **Start onboarding** on the sandbox page. The CLI says so with the
  page's URL; the start endpoint refuses CLI tokens (403). Adding a repository in the web UI still
  starts onboarding at once.
- Dead-code tools (SPEC-054, ADR-0037, task #38): a bundled skill, `dead-code-cleanup`, is how
  agents clean up: the established tool per language (vulture and skylos for Python, from the ML
  in PL workshop; cargo-shear, knip, deadcode and others), a web search when the stack is not
  listed, every candidate verified, small commits. Onboarding and `infra-bootstrap` add a
  dead-code check to a project's gate; the Chore workflow points to the skill. Yulius's own
  `scripts/dev deps` runs cargo-shear instead of cargo-machete and dropped the unused `toml`
  workspace dependency it found.
- "See the original prompt" in the task menu (SPEC-043, ADR-0033, task #42) shows the prompt a
  task started with: the text formatted as written, with Copy text, and the files attached to it
  (an image as a thumbnail that opens full size, another file to download, a file removed since
  named). `GET /api/tasks/{tid}/prompt` returns it; `GET /api/tasks/{tid}/prompt/attachments/{n}`
  serves a file only from the task's own attachment folders, never through a link or from a path a
  prompt names elsewhere.

## 2026-10-12

- Armed merges survive a server restart (a deploy no longer drops them) (SPEC-050 DLV-205).
- Secrets are a title, a value and notes, and reading one needs your own two-factor code
  (SPEC-040, ADR-0042, task #39). The first reveal sets two-factor up: an authenticator key and
  link, confirmed by a first code. After that, a code unlocks the vault in that browser for 10
  minutes. A sign-in or password alone no longer reveals anything. Older team secrets keep every
  field: the password becomes the value, and the address, user name and seed move into the
  notes. Agent secrets lose the per-secret seed and take a user name only for Basic credentials.
  The people who manage an agent secret can now read it after a code. Agents still never see it.
  Ten wrong codes in a row lock codes until the person confirms it is them again. Rolling back
  past this change needs care: the previous release reads secrets saved in the new form as
  having no password, and an edit there would drop the value.
- The Cockpit is now the **Agent**, on the right (SPEC-035, ADR-0036; task #33): a top-bar Agent
  button and the first tool of the right rail open the user's own Claude Code session in the tool
  panel beside the page, under a notice that it is for quick work (setting up automations, quick
  questions) and that anything more complex, such as writing code, belongs in a task (with a New
  task link). It is no longer in the left sidebar; `/s/<slug>/agent` and the old
  `/s/<slug>/cockpit` open the panel. The API and server keep the `cockpit` names.
- A sandbox's default memory limit is the host's memory but a reserve (an eighth, at least 4 GiB),
  not half: at half, ten sessions and two CI lanes thrashed the page cache while the host had 20 GB
  free, and CI jobs timed out (SPEC-050 MEM-101).
- The onboarding prompt tells the truth about Yulius CI (SPEC-020 ONB-001, task #34): the jobs'
  home and `$YULIUS_CI_CACHE` are their own, start empty and persist across runs (they were
  described as "an empty home"), so a job installs what it needs and never relies on an earlier
  run; the agent proves it by running each job normally and in a clean checkout with a fresh
  `HOME` and CI's environment. The `infra-bootstrap` skill says the same in its CI step. CI itself
  is unchanged.
- Menus are compact and filter themselves (SPEC-038, ADR-0030; task #36). Every drop-down stays
  inside the window and scrolls its items. Descriptions take one line, with the full text as a
  tooltip. A menu of 8 or more items opens with a focused Filter box: typed words narrow it,
  ↓/↑ and Enter pick, and Escape clears the filter first. "Run after" lists every live task on one
  line ("#12 Checkout API · Working") under "After a task is done", not just the first 20.
- Notes in the tools rail (SPEC-041, ADR-0032, task #40): a notepad for this sandbox and one for
  the team, below Shell. They save by themselves while people type, with no Save button. Everyone
  who can see the sandbox reads its notes, writers edit them, and everyone signed in shares the
  team's. A teammate's edit appears without a reload. When two people type at once, the later save
  is refused and that person picks Keep mine or Use theirs. API: `GET/PUT /api/notes` and
  `/api/sandboxes/{sid}/notes`, event `note.updated`.
- A pull request runs its own CI jobs only when it changed them itself; a branch that is merely
  behind `dev` runs `dev`'s current jobs (SPEC-050 CI-110).
- `yulius wait` says why Yulius has not pushed or opened the pull request at the end of a turn (an
  open step, a missing plan, an invalid spec, uncommitted work) instead of waiting in silence (SPEC-050 DLV-204).
- End-to-end tests in CI get longer budgets (90 s per action, 6 min per test) and two retries: they timed out while agents' builds loaded the machine, though the same tests pass locally in seconds; CI CPU priority is #283.
- Armed pull requests stay current by themselves (SPEC-050 DLV-203): when the integration branch
  moves on, Yulius merges it into the idle task's branch (the repository's merge drivers apply, so
  a shared changes log no longer conflicts) and pushes a fast-forward; the merge stays armed for
  the new head. A real conflict is left to the agent with the files named.
- Tasks run in parallel and finish without obstacles (SPEC-050):
  - a Done task no longer holds a session slot, so queued tasks start;
  - when Yulius cannot push an agent's head, `yulius wait` says why and how to fix it without a
    force push, and agents bring `dev` in by merging, never by rewriting pushed commits;
  - a shell that only runs `yulius wait` no longer counts as a build, so a waiting task reads as
    waiting and its caches can go under disk pressure;
  - agent commands run below CI and the agents in CPU priority;
  - shells and the Cockpit keep GitHub across server restarts;
  - a sandbox runs two CI runs at once, each lane with its own job checkouts, and worktree
    registrations change one at a time (#206).
- A Done task can be deleted (SPEC-039, ADR-0031, task #37). "Delete permanently…" closes its
  idle session (`closed`, as Archive does), then deletes the task. Only a task that is still at
  work or waiting for you has to be stopped first, and its menu says "Still running: stop the
  task before deleting it".
- Two ready-made automations, enabled in one click on the Automations page (SPEC-042, ADR-0034,
  task #41): "Keep the docs up to date" (the last day's changes, or all docs on the first run)
  and "Work through open issues" (the core team's open issues verified in parallel by
  subagents, a Fix task for each actionable one, never a new issue), both nightly at 03:00 CEST.
  They live in `catalog/automations/`. Tasks now report a bug they confirm with `issue_create`
  unless an issue already tracks it (briefing and the Fix workflow). Neither runs until a person
  enables it; on the team's instance both exist, disabled.
- CI runs are no longer refused over the sandbox's disk quota by the size of test builds
  (SPEC-033, ADR-0028, task #31): the `yulius` crate's 50 integration test files, each its own
  190 MB executable linking the whole product, are now modules of one test binary
  (`tests/main.rs`); a gate build's test executables drop from 9.2 GB to 1.3 GB. The repository
  check rejects a second test binary or an undeclared test file. Run one file's tests with
  `cargo nextest run -p yulius -E 'binary_id(yulius::main) & test(/^<file>::/)'`.
- A long task title in the start-rule editor stays on one line ending in an ellipsis instead of
  wrapping over the condition's border (SPEC-031, task #29).
- The Cockpit and task agents create tasks and automations (SPEC-037, ADR-0029; task #35). A new
  `task_create` MCP tool creates a task the way New task does (workflow, mode, after a task or a
  start rule), on the authority of the task's creator; tools create at most 20 tasks an hour
  per project. The Cockpit now starts with the Yulius MCP server under its own bearer. It is
  served the tools for tasks, automations, workflows and sessions, acting for its owner, so
  "create a task from that" or "every day at 4am align the docs" just works. It asks before
  creating anything. A Cockpit already running gets the tools when it starts again.
- Attached images reach Claude Code as pasted images (SPEC-030, ADR-0026; task #28). A task with
  attached images starts with no CLI prompt. Yulius types its first message into the terminal:
  each image's path is pasted (Claude Code shows `[Image #N]` and sends an image block), then the
  text with the other files mentioned. Before this, an `@` mention showed the path and reached the
  model as a Read tool call.
- The sandbox navigation is shorter (SPEC-029, ADR-0025, task #27): Cockpit, Tasks, Git,
  CI (tests), Secrets and Dashboard, then "More", which expands Sessions, Workflows, Skills,
  Automations and MCP in place. Its state is remembered per browser; arriving at one of those
  pages shows it open for that page.
- A task's status follows what its agent is doing (SPEC-032, ADR-0044; task #30): a task whose
  agent ended its turn but still waits for a background agent (a reviewer) is Working with "1
  background task still running", not Done; a goal check that says "not met", or any stop the
  session goes on from, makes the task Working again instead of leaving "Your turn" (or Done);
  informational Claude Code notices (a background agent finished, push, quota resume, …) no longer
  flag "Needs your action", and the agent's next work takes a real flag back once answered; a
  restart no longer demotes a working task whose turn a Stop hook continued. Monitor streams are
  still not counted.
- A usage limit no longer types "Continue." into a task (SPEC-034, decision 0027, task #32): the
  session moves to another subscription (or waits for the reset) and resumes as if the limit never
  happened. The transcript steps back over the limit errors and notices, and Claude Code redoes
  the lost turn by itself: it resubmits the unanswered message, or goes on after the last tool
  result.
- Tasks run Opus 5.5 at high effort by default (SPEC-023, task #21): a task without a model runs
  `claude-opus-5-5` instead of Claude Code's moving `opus` alias, and a task without an effort runs
  at `high` (none for Haiku); the Cockpit starts on them too; the composer and Settings say so.
- Messages are gone for now (SPEC-028, ADR-0023; task #26): no `message_send` or `messages_inbox`,
  no stored notices, notice digest or "unread messages" stop reason, nothing typed into an idle
  session, and no Messages panel, unread counts, message notifications or
  `GET /api/tasks/{tid}/messages`. Agents learn about their checks, delivery and armed merges from
  `yulius wait` (which now says how an armed merge ended), `ci_status` and `delivery_status`. The
  stored messages stay as history. A `yulius wait` already running when the server updates exits
  with "cannot be reached" once (its answer changed shape); start it again.
- A file attached on the new-task composer opens when its chip is clicked (an image in a preview,
  another file in a new tab), and the task's Claude Code session receives attached files natively
  as `@path` mentions in its first message, an image as an image, instead of paths to open with the
  Read tool (SPEC-025, decision 0022).
- A half-written new task is kept (SPEC-026, task #24): its prompt, attached files (with thumbnails)
  and choices come back after visiting another page or reloading, per sandbox; starting the task
  clears it, signing out removes it.
- Every user has a Cockpit, first in the sandbox navigation: their own plain Claude Code session in
  the sandbox (no Yulius hooks, MCP or briefing) on a pool subscription, for quick questions and
  work that should not become a task. `/clear` starts over; it cannot be deleted, and after an
  exit, a day unwatched or a restart it resumes its conversation (SPEC-021, ADR-0020, task #19).
- A start rule is picked, not typed: the composer's "When a rule is true…" opens an editor of
  conditions (a task of the sandbox and "is done", "is merged" or "has ended") joined by All or
  Any, shows the sentence it builds, opens again with what was picked and is not offered in a
  sandbox without tasks; the API still takes the full rule language ([SPEC-022](../.ai/specs/SPEC-022-when-rule-is-true-should-be-a-simple-edi.md), ADR-0019, #20).
- Claude Code sessions in a task terminal no longer open Claude Code's diff side panel by themselves
  ("No changes this session" on the right); it still opens by hand within a session (SPEC-024).
- A click on Claude Code's output in a task terminal reaches Claude Code, so a collapsed tool
  group expands as in a native terminal; drags and double-clicks still select text (SPEC-027).
- Ended worktrees holding only end-to-end output (`.e2e`), the APM cache (`apm_modules`) or Claude
  Code's own `.claude/` state are removed too (SPEC-019; seven were kept on the first deploy).
- Agents meet fewer avoidable obstacles (SPEC-020):
  - pinned tools are built once per home and linked into each worktree;
  - the changes log merges with Git's union driver;
  - memory pressure kills an agent's build before the agent, and agent Rust builds use a third of
    the CPUs;
  - CI reuses the result of a tree it already tested, stops a run's jobs when one fails, keeps a
    superseded pull request's queue place, retries an errored run once and says it is not the code,
    and tests a pull request with its own CI jobs;
  - proactive switches wait for background shells;
  - Yulius gives each task its spec number;
  - onboarding runs as a goal task that must run every CI job.
- Nothing refers to GitHub Actions runs any more: the delivery steps, the pull request template,
  the `yulius-git` and `yulius-verify` skills and the branch policy's promotion evidence use the
  `yulius/ci` status of the exact dev head (#184).
- Tasks finish on `dev` by themselves (SPEC-011; tasks #10, #12, #16): a goal task starts from a
  request file (the request, its attached files to open, the definition of done) and a `/goal`
  that names it, typed once Claude Code's session started and confirmed, so long requests get a
  goal and attached images reach the model; the standard profile has no manual `dev` checkpoint;
  `delivery_merge` while the checks run arms the merge and Yulius merges when they pass; agents
  wait with `yulius wait` instead of polling; the briefing and notices end at the merge into
  `dev` (never promote unless asked) and fit Claude Code's hook limit; tasks run Opus by default
  with auto-compact on and 10-minute Bash timeouts.
- Tasks can start by a logical rule: besides "run after #N", a new task can wait for a sentence such as `#12 done and #13 done` or `#12 done or #14 ended` (composer: Run after → "When a rule is true…", API `start_rule`); it starts the moment the sentence is true and goes to Needs you when it never can be ([SPEC-017](../.ai/specs/SPEC-017-task-start-rules.md), ADR-0018).
- Sandboxes stay clean and background shells stay honest (SPEC-019): an idle task's caches go after
  4 hours whatever the quota, never while its background shells still run a build; ended tasks'
  worktrees go when only rebuildable directories are ignored; orphan worktrees and removed
  sandboxes' worktrees and CI checkouts are pruned after a day; a Bash `pgrep -f`/`pkill -f` that
  would match itself is refused before it runs; an idle agent is told once when its background
  shells run 30 minutes, and the UI shows their age.
- Task-to-task messaging is removed for now: `message_send` only tells or asks people (`to="people"`), `scope_claim` shows an overlap to the claiming agent only and messages nobody, and the briefing, digest and Messages panel speak of Yulius's notices. Notices, the inbox, the digest and the idle wake-up are unchanged (SPEC-012, COM-002 withdrawn).
- A task's state reads the same everywhere and matches what it waits for (tasks #11, #13, #14):
  an idle agent whose background shells still run is Working, not "Your turn", and the task view
  no longer asks for a reply then; a task being stopped is listed under Stopped at once; any task
  shown as Done (goal achieved, or plan complete with its pull request merged or tested) can be
  archived, and its session is closed rather than stopped (SPEC-001 TSK-014, UI-050).
- Merging into `dev` no longer waits for a person: the `standard` delivery profile has no manual `dev` checkpoint, so after the merge on green checks the next action is promotion; stored profiles with the old checkpoint read as the new one (SPEC-011 DLV-009, DLV-010, #10).
- GitHub Actions are gone from this repository: Yulius CI is the only CI (`quality` and `e2e` in
  `[[ci.jobs]]`), the rulesets require only the `yulius/ci` commit status, and `.github/workflows`
  and `.github/actionlint.yaml` are deleted. macOS is checked with `scripts/dev ci` on a Mac before
  promotion; the branch policy is checked by hand until it joins Yulius's merge gate (#223). The
  `ci_setup` tool and the CI page still handle other projects' workflows (#184).
- A subscription whose only task is running but idle is read again: the usage round skipped every
  subscription with a running task, but an idle session reports nothing, so its reading went stale
  (5.6 h on production); it now skips only fresh readings (SPEC-007 USG-002, #4).
- Agents wake on new messages again: Yulius typed its notice and Enter in one write, which Claude
  Code read as a paste with a newline, so the notice never submitted and piled up in the input box;
  Enter now follows separately, and an unsent notice is only submitted.

## 2026-10-11

- A full sandbox disk no longer ends running sessions or needs a person: a switch checks that the new session can start before it ends the running one (the reason shows in the terminal and the switch log); a refusal over the quota measures again when its reading is over a minute old; Yulius removes build caches (Cargo `target` and similar, or `[disk] caches`) of tasks that are not working and the worktrees of closed and cancelled tasks, prunes idle tasks' caches from 90 % of the quota, and tells the agent what was freed; persistent CI job caches are pruned too, at once beyond a quarter of the quota ([SPEC-008](../.ai/specs/SPEC-008-sandbox-disk.md)).

- Agent primitives follow the agentic-coding workshop methodology (SPEC-005, ADR-0016): repository
  skills are APM packages (`.apm/skills` source, `apm.yml`, `apm.lock.yaml`, generated
  `.agents/skills` and `.claude/skills`), the repository check rejects deployment drift and skills
  without a "Use when/for" sentence or over 200 lines, a primitive design standard
  (`docs/engineering/primitives.md`) and the `yulius-primitives` skill apply the pattern
  vocabulary, and `infra-bootstrap` now audits a project's conventions into primitives.
- A green pull request behind its base (the repository requires up-to-date branches) is not
  mistaken for a missing permission: the task is told to merge the base, and `delivery_merge`
  says why it refuses.
- A task whose pull request conflicts with its base is told to merge the base and resolve it;
  GitHub runs no checks on a conflicting pull request, so the agent would otherwise wait for them.
- Debug and test builds keep only line tables (dependencies none): a worktree's test build had
  reached 48 GB and parallel tasks filled the sandbox disk quota, so CI runs and session starts
  were refused.
- Agents carry delivery through on their own: Yulius tells a task when its pull request opened and
  when its checks passed or failed, the Stop gate asks for a missing spec at once (and follows a
  renamed one), and the briefing says never to ask a person — an issue covers what only a person
  can do.
- Yulius CI `quality` job failed at once on a clean machine (no rustup): `scripts/bootstrap.py`
  installs it when the run is unattended or given `--install-rustup` (no default toolchain, no
  profile edits).
- Everything a task does on GitHub shows the person who created it: commits with the no-reply
  address GitHub links to their account, and the agent's Git and `gh`, Yulius's pushes and pull
  requests, the agent's merges, promotions and issues with their own token; the App only when
  they have none, said once in the task's terminal (SPEC-001 GIT-003, #188).
- The GitHub CLI is logged in as the user in sandbox shells and editors: a `gh` shim fetches the
  user's own GitHub token per call (nothing stored on disk), like Git's credential helper
  ([SPEC-004](../.ai/specs/SPEC-004-gh-cli-login.md)).
- Subscriptions show usage all the time: for a sign-in the usage endpoint refuses, an idle
  subscription is read from the rate-limit headers of a one-token request every few minutes
  instead of waiting for a task (ADR-17, #4).
- Repository prepared for Yulius: `yulius.toml`, product summary, architecture, decision
  records, changes log; GitHub Actions jobs translated into Yulius CI
  (the workflows stay until the GitHub App may post commit statuses and change workflow files).
- Tasks wait for preparation instead of being refused (#175); onboarding survives multi-byte
  text and titles are the first real prompt (#173); Secrets adds from its header and popups
  close on an outside click (#171).
