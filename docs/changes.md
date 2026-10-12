# Changes

Notable changes, newest first. Per-change detail lives in the merged pull requests and in the
evidence and changelog sections of [SPEC-002](../.ai/specs/SPEC-002-yulius-architecture.md)
(sections 14 and 15).

## 2026-10-12

- The sandbox navigation is shorter (SPEC-029, ADR-0025, task #27): Cockpit, Tasks, Git, CI,
  Secrets and Dashboard, then "More", which expands Sessions, Workflows, Skills, Automations and
  MCP in place. Its state is remembered per browser; arriving at one of those pages shows it open for that page.
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
