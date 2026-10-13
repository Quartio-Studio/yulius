# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-13

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

- Secrets are a title, a value and notes, and reading one needs your own two-factor code
  (SPEC-040, ADR-0034, task #39). The first reveal sets two-factor up: an authenticator key and
  link, confirmed by a first code. After that, a code unlocks the vault in that browser for 10
  minutes. A sign-in or password alone no longer reveals anything. Older team secrets keep every
  field: the password becomes the value, and the address, user name and seed move into the
  notes. Agent secrets lose the per-secret seed and take a user name only for Basic credentials.
  The people who manage an agent secret can now read it after a code. Agents still never see it.
  Ten wrong codes in a row lock codes until the person confirms it is them again. Rolling back
  past this change needs care: the previous release reads secrets saved in the new form as
  having no password, and an edit there would drop the value.
- The Cockpit is now the **Agent**, on the right (SPEC-035, ADR-0034; task #33): a top-bar Agent
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
- A task's status follows what its agent is doing (SPEC-032, ADR-0034; task #30): a task whose
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
