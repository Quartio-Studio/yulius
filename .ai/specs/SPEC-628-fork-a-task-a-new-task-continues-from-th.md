---
yulius-task: 96eea568-73fb-4b39-8179-d2211f150f51
status: done
---

# SPEC-628. Fork a task: a new task continues from the same conversation, separately

## Goal

A person, or the Superagent, forks a task: a new task of the same project and harness starts
from the source task's conversation and committed work, and then goes its own way. Both continue
separately. The owner approved this on 22 Nov 2026 after a review of iTermX, whose Clone Tab runs
`claude --resume <ID> --fork-session` in the same folder and account.

## Context

- Today the only way to try another approach from where a task stands is a new task with a long
  hand-written brief. The brief loses the context.
- A task's Claude Code transcript is `<sandbox home>/.claude/projects/<slug of its worktree>/
  <session>.jsonl` (`yulius_harness::paths`). Codex rollouts live in the sandbox's shared
  `codex/sessions` (SPEC-503 CDX-003). Antigravity conversations live in the sandbox's shared
  `brain` (SPEC-505 AGY-003).
- Continuing an imported session already copies a stored transcript into a task's worktree
  project folder, with every record's `cwd` rewritten (`imports::place_claude_transcript`,
  IMP-004). A chained task already starts its worktree from another branch (`chain_base`,
  TSK-012). The fork reuses both.
- Probed on 2026-11-22 with the real Claude Code 2.1.330 against a local stub API
  (`$HOME/.cache/yulius-task-612/probe/fork_probe.py`):
  `claude --resume SRC --fork-session --session-id NEW <prompt>` in another folder, with SRC's
  transcript copied there and its `cwd` rewritten, sent SRC's conversation and the new prompt to
  the API. It wrote `NEW.jsonl` with every record under the new session id. Copied records kept
  their original timestamps, and SRC's file stayed byte-identical. `--session-id` with
  `--resume` is refused without `--fork-session` ("also needs --fork-session").
- Codex 0.170.0 (pinned) has `codex fork [SESSION_ID] [PROMPT]`, with the same options as
  `codex resume`. It writes a new rollout with a new session id. Codex has no `--session-id`.
- Antigravity CLI 1.5.12 (pinned) has no launch flag to fork. Its help lists `--conversation`
  (resume) and `--continue`. Its `/fork` exists only as a command typed into a running session,
  which first opens the source conversation in a second instance.

## Requirements

| ID | Requirement |
| --- | --- |
| FRK-001 | `POST /api/tasks/{tid}/fork` (body `ForkTask`: optional `message`, `title`, `model`, `effort`, `account_id`) and the MCP tool `task_fork` create a fork of a task the caller may work on, in the source's project. The fork has the same harness, a new number, branch and worktree, and its own spec path, given at its first launch as for every task. It copies the source's request, mode, workflow and steps (titles, statuses and notes). It starts at once, as a new task does (onboarding gate, queue, capacity). |
| FRK-002 | Same harness only. Model and effort are the harness defaults unless the caller gives them; a model of another harness is refused. The subscription is the pool's choice unless the caller names one of the harness's provider. |
| FRK-003 | Refused, with nothing created: a source without a recorded session ("#N has no conversation to fork yet"), an Antigravity source ("Antigravity cannot fork a conversation at launch: its /fork works only inside a running session"), a source without a branch commit, and a message over 64 KiB. |
| FRK-004 | At fork time Yulius records where the fork came from: the source task (id, number, title), its branch, its head commit (the branch's commit in the sandbox repository) and its latest session id. For Claude Code it also copies the source transcript's complete lines (and its folder of side files, bounded as a continued session's are) to `<data>/transcripts/<fork task id>/`. The source is only read: its record, steps, branch, worktree, transcript and process stay as they were, and its session keeps running. |
| FRK-005 | The fork's worktree is a new branch at the recorded commit, so it holds the source's committed work. Uncommitted changes in the source's worktree do not come with it, and the opening message says so. |
| FRK-006 | The fork's first launch continues a copy of the source's conversation. Claude Code: the stored copy is placed in the fork worktree's project folder with `cwd` rewritten, then `claude --resume <source session> --fork-session --session-id <new id>`. Codex: `codex … fork <source session> -- <opening>`; the new session id is adopted from its SessionStart hook or rollout, as a new Codex session's is. The source's session id never enters the fork's lineage. Later launches resume the fork's own session as usual. |
| FRK-007 | The fork's opening message names its task number, branch and spec path, the source task and its commit. It says that the source goes on separately and that the source's branch, pull request and spec are not the fork's. It ends with the person's message, or "Continue from where the conversation stopped." In goal mode the goal is set after that first turn, as for a resumed session. |
| FRK-008 | `TaskSummary` carries `forked_from` (task id or null once deleted, number, title, commit, session) and `forks` (the forks' id, number, title and status). Task pages show both. Deleting the source keeps its forks' records, with the number and title. Deleting a fork deletes its record. |
| FRK-009 | The task menu and the task page have a Fork action. Its dialog takes an optional message saying what to do differently, and optional model and subscription. It opens the new task. The action is disabled, with the reason, for Antigravity tasks and tasks without a conversation. |

## Plan

- [x] Store task forks (migration 0092): record, read, list a source's forks, keep them past a deleted source
- [x] Harness forks a session: Claude Code --fork-session with --session-id, Codex fork, Antigravity refused
- [x] Fork use case: same-harness checks, source head commit, transcript snapshot, task with the copied plan
- [x] Fork launch: worktree at the source commit, transcript placed for the fork, opening message, own lineage
- [x] Fork API, task_fork MCP tool and fork origin and forks in task summaries
- [x] Tests that fail without the fork: fixture conversation carried, branch at the source head, source unchanged
- [x] Docs for the backend: architecture, api, ADR-0463 and the changes log
- [x] Independent review of the backend pull request
- [x] Fork action in the task menu and on the task page, with its dialog
- [x] Fork origin and forks shown on both task pages
- [x] End-to-end spec of the Fork dialog and the origin shown on both pages
- [x] Product docs and the changes log for the UI; independent review of the UI pull request
- [x] Agent shim passes yulius run, which the dev gate's session-command test needs (staging fix forward)

## Acceptance criteria

- A fork of a Claude Code task launches with `--resume <source session> --fork-session
  --session-id <new>`. The transcript its agent reads holds the source's fixture conversation,
  with `cwd` set to the fork's worktree.
- The fork's branch is at the source branch's head commit at fork time. A later commit on the
  source does not move it.
- The source's record, steps, branch head, transcript bytes and running process are unchanged.
- A Codex fork launches `codex … fork <session> -- <opening>`. An Antigravity fork, or a fork
  of a task without a session, is refused and creates nothing.
- Both pages show the link; the API and the MCP tool return the fork.

## Test plan

- `yulius-store` tests: insert and read a fork, list a source's forks, delete the source (link
  kept, id null), delete the fork (record gone).
- `yulius-harness/tests/launch.rs`: Claude, Codex and Antigravity fork plans.
- `crates/yulius/tests/forks.rs` (new), with the fake agent's new `--fork-from <transcript>`
  doing what the real CLI does (copy the source records under the new session id): the fork
  carries the fixture conversation, its branch is at the source head, the source is unchanged,
  and refusals create nothing. The MCP tool goes through `task_fork`.
- `e2e/specs/fork.e2e.ts`: the menu and page action, the dialog, both pages' links.
- The real Claude Code probe above, rerun for the evidence.

## Decisions

- **Snapshot the Claude transcript at fork time and resume a placed copy.** The probe showed
  Claude Code 2.1.330 also finds a session in another project folder. Relying on that would make
  the fork read the source's live file at its own launch time, not at the recorded commit, and it
  would leave the old `cwd` in the history. The copy is the state at the commit Yulius recorded.
  It is what continuing an imported session already does.
- **`--session-id` with `--fork-session`.** Yulius names the fork's session itself, as for a new
  session. The lineage then holds it from the spawn and never the source's id.
- **The launch is a resume of a copy.** `Resume { fork }` keeps the resume behaviour: the opening
  message is the first turn, the goal is set after it, and the transcript is watched from the run's
  start. That keeps the source's copied history out of the fork's usage. A `New` launch would set
  the goal first and lose the message.
- **Codex reads the rollout when the fork starts.** Codex has no way to name a fork's id or to
  resume a copy under another id, so `codex fork` reads the shared rollout at launch. A fork that
  waits for capacity can therefore see turns the source took after the recorded commit; the spec
  says so rather than rewriting rollout ids.
- **Antigravity is refused.** Typing `/fork` into a second instance opened on the source
  conversation would race the source's own instance, which the CLI itself warns about.
- **A table of its own (`task_forks`).** A fork is a link between two tasks, like a chain. A
  column set on `tasks` would change every task literal for an optional fact.
- **Forking needs to see the source and to work in the project.** The fork changes nothing of
  the source, and the new task is the forker's own. So it asks for view access to the source
  (not its owner's control) and work access to the project, as New task does.
- **Two pull requests.** The backend (store, harness, use case, API, MCP, tests, docs) first,
  then the web UI, each within the size rule.

## Evidence

Backend pull request (Plan items 1–7):

- Real Claude Code 2.1.330 against a local stub API (`$HOME/.cache/yulius-task-612/probe/fork_probe.py`):
  a session in folder A, its transcript copied to folder B's project folder with `cwd`
  rewritten, then `claude -p --resume SRC --fork-session --session-id NEW <prompt>` in B. The second
  API request held A's user message and the new prompt (`users=2`). `NEW.jsonl` held every record
  under the new id, and the copied records kept their timestamps. SRC's file stayed byte-identical
  (`source unchanged True`). Without the copy (`nocopy`) 2.1.330 also found SRC in A's folder.
- Codex 0.170.0 (pinned, from npm): `codex fork --help` shows
  `codex fork [OPTIONS] [SESSION_ID] [PROMPT]`. Antigravity 1.5.12 (pinned): `--help` lists no
  fork flag; `/fork` appears only as an in-session command in its strings.
- `scripts/cargo nextest run -p yulius-store -p yulius-harness -p yulius-app`: 1138 passed.
- `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main)'` (after merging dev):
  2621 passed, 24 skipped.
- New tests: `yulius-store` `fork_links_persist_and_outlive_a_deleted_source`;
  `yulius-harness/tests/launch.rs` `claude_fork_resumes_a_copy_under_a_new_session_id`,
  `codex_fork_argv`, `antigravity_fork_is_refused`; `forks::a_fork_continues_the_sources_conversation_and_branch_and_leaves_the_source_alone`,
  `forks::forks_that_cannot_continue_are_refused_and_create_nothing`;
  `manage_tools::task_fork_refuses_a_task_without_a_conversation`,
  `manage_tools::a_writer_may_fork_another_persons_task_and_a_reader_may_not`;
  `forks.rs` unit tests for the complete-line copy and the opening message.
- Mutation checks by hand: each was undone and then restored. Without the fork base in
  `ensure_worktree`, the fork's HEAD was not the source's head (forks.rs:240). With the copy not
  placed, the fork's agent could not read the conversation ("fork run"). With the fork arm of the
  launch disabled, the launch was `New` (forks.rs:195). Without
  `UPDATE task_forks SET source_task_id = NULL`, deleting the source was refused ("conflicting
  record"). With owner control asked of the source, a writer's fork of another person's task got
  403.
- `scripts/dev quick` passed. The security scan is switched off for this project in Yulius
  (`yulius security-scan` prints "Security scan disabled"): not run, not passed.

Review round 1 (Fable 5.2, head `9ae710d21`): request changes, 3 major, 4 minor, 2 nits, all own
code. Answered:

- The named subscription was lost in a queue wait, and the queue could start a fork before its
  record existed. A fork is now inserted `prepared`. `start_fork` writes its record, copy and plan,
  then `queue_task` moves it from `prepared` to `queued` with its start and subscription in one
  step. A failure removes the task, the stored copy and the staged copy. Tests:
  `forks::a_codex_fork_waits_with_its_subscription_then_resumes_its_own_session` (kept
  `account_id`, the run on the named subscription after the source freed the only slot) and
  `forks::a_queue_pass_before_the_forks_record_starts_nothing` (race point `ForkInserted`, a
  `drain_queue_now` in between leaves it `prepared` with no run).
- Relaunch paths: the Codex test continues the stopped fork, and the second launch resumes the
  fork's own session with `fork: None`.
  `forks::a_fork_whose_first_launch_failed_launches_as_a_fork_again` covers a launch failure
  before any process: the lineage stays empty and the next launch is a fork again.
  `forks::the_task_fork_tool_forks_a_task_of_the_project` covers a successful `task_fork`. The
  refusal test adds another provider's subscription and an unknown one (429). It now counts
  archived and other tasks, and it checks that nothing was staged.
- Minor: the copy runs in 64 KiB chunks and is cut after its last newline, with a unit test for
  lines longer than a chunk. The steps are read before staging. `fork_base` returns store
  errors. The fake copies the forked transcript before it prints READY. Nits: the refusals now say
  that a branch or transcript can also be gone after a task ended.
- The reviewer's real Codex run, quoted: "**Codex 0.170.0**, with Yulius's argv order
  (`--no-daemon -C … -m … fork <id> -- <prompt>`): the request carried the source conversation
  plus the new prompt. The SessionStart hook named the new id (`source: "fork"`), and the source
  rollout stayed byte-identical."
- Mutation checks by hand: without the subscription in the kept start, the Codex test failed
  (forks.rs:477). With the fork inserted `queued`, the race test failed.
- Merged origin/dev (brokered commands): migration 0092 is schema version 89, and the Cockpit
  serves 86 tools. dev's `a_cockpit_reaches_the_yulius_tools_with_its_token_only_in_its_environment`
  failed on dev itself: `commands_list` had been added without the test's expectation. It is fixed
  in its own commit.
- After the fixes: `scripts/cargo nextest run -p yulius-store -p yulius-app -p yulius-harness
  -p yulius-contracts -p yulius-server`: 1227 passed. `scripts/cargo nextest run -p yulius -E
  'binary_id(yulius::main)'`: 2636 passed, 24 skipped.

Review round 2 (Fable 5.2, head `665a45f73`): approve, all round-1 findings resolved. The notes
left after it:

- The cleanup branch had no test. `forks::a_fork_whose_record_fails_leaves_nothing_behind` deletes
  the source at `ForkInserted`, so the record's link fails, and checks that the fork task, its
  stored copy and the staging folder are gone. Without the task delete in `start_fork`'s
  cleanup, it failed.
- The short `prepared` span at a fork's start is now stated in the architecture note.
- The long JSON literal in the Codex test is split with `concat!`.
- Not changed: when `queue_task` finds that someone already started or cancelled the fork in the
  milliseconds before it is published, the fork is removed. The task is not yet shown to anyone at
  that point, so no person's action is lost.

Review round 4 (Fable 5.2, head `d224b2645`, the dev merge with SPEC-625): request changes, one
major. `task_fork` let a task agent fork a personal peer whose transcript ADR-0460 withholds from
it. `tool_task_fork` now asks `require_reader` and `require_transcript_reader` before anything is
staged. Test: `peer_status::a_personal_tasks_conversation_is_not_forked_by_a_task_without_its_secrets`
(bob's agent and an automation's are refused with that sentence and nothing is created; alice's
other task passes the check). It failed before the change ("no conversation to fork yet").

The backend was merged into dev as #1706 (`6611b4d54`), from PR head `f7504a134`, on green Yulius CI.

UI pull request (Plan items 9 to 12):

- `actions.js`: Fork… sits in the task menu, both on the page and on a sidebar row. It is off,
  with the reason in its name, without write access, for an Antigravity task, and for a task that
  has not started. Its dialog asks what to do differently, a model of the task's harness and a
  subscription of its provider. It says the source goes on untouched, uncommitted changes stay
  behind, and the new task and its copy of the conversation are the forker's. It then opens the
  fork.
- `taskview.js`: the strip shows a "Fork of #N" chip linking to the source, with the commit in
  its title, and a "Forked: #M" chip for each fork. `pages/taskinfo.js`: a Fork… button beside
  Review changes, and "Forked from" and "Forks" rows. The mock serves
  `POST /api/tasks/{tid}/fork`, `forked_from` and `forks`.
- `cd e2e && npx e2e run tests/fork.e2e.ts`: 3 passed. These cover the menu's dialog (no WCAG
  violations), the message sent, the fork opened with its chip, the source's chip back, the
  details page's button and Forks row, and the refusal reasons for an Antigravity task and a
  queued one. The neighbouring specs (menus, ownership, consistency, git, handoffs, archive):
  80 passed.
- `ui_contract` (route literals against the router): 12 passed.
- Round-5 nits: the same person's task's pass in
  `peer_status::a_personal_tasks_conversation_is_not_forked_by_a_task_without_its_secrets` now
  asserts "no conversation to fork". `task_fork` joins the disabled-owner case of
  `peer_status::a_task_reads_only_what_its_owner_may_view`. With `require_reader` removed, that
  case still passes, because `manage_authority` refuses a disabled person first. That check stays
  as the reviewer called it: defence in depth.

UI review round 1 (Fable 5.2, head `3bcd4bcf5`): request changes, 1 major, 4 minor and 3 nits in
own code, and 1 pre-existing. Answered:

- Major: the source's page learned of its fork only through an event the mock sent and the server
  did not. The server now publishes the source after a fork, and a deleted task's source and forks
  (`fork_neighbours`, in `delete_task_as`). The client takes `forked_from` and `forks` from every
  summary and detail, with no stale value kept when they are absent (`pickSummary`,
  `lifecycleSummary`, the task view's `update`). Test:
  `forks::a_fork_and_its_deletion_publish_the_other_task` (the source is published with its fork,
  then without a deleted fork, and the fork is published with its deleted source's id gone). It
  failed before the change ("the source is published with its fork: []").
- Minor:
  - `forkWhy` refuses a `not_started` task.
  - A refusal shows inside the dialog, through `openModal`'s error line.
  - The message is counted in bytes (64 KiB) in the dialog and in the mock.
  - The details page's Fork… uses `aria-disabled`, with the reason in words beside it
    (`aria-describedby`).
  - The mock's delete clears its source's and forks' links.
- e2e (`tests/fork.e2e.ts`, now 4 tests): a `not_started` task's Fork… is off with its reason; the
  details page's off state shows its reason, opens no dialog from the keyboard and has no WCAG
  violations; a refusal is said in the dialog; deleting the source leaves the fork a plain "Fork of
  #4" chip whose title says "(deleted since)".
- Nits: the mock answers 200, and its other-provider refusal says "cannot take the fork".
- Pre-existing: `openModal` leaves focus on the body after a failed action. Filed as
  [#1714](https://github.com/Quartio-Studio/yulius/issues/1714); it is outside this change, since
  `openModal` is unchanged on dev.
- After the fixes: `scripts/cargo nextest run -p yulius -E 'binary_id(yulius::main)'` 2671 passed,
  24 skipped; `-p yulius-app -p yulius-server` 749 passed; the fork e2e spec 4 passed; menus,
  ownership, consistency, git and archive 75 passed.

UI review round 2 (Fable 5.2, head `628f488a0`): approve; the round-1 findings are resolved with
tests. One test-only nit is left: the dialog's 64 KiB byte check has no e2e case. The reviewer
observed it by probe: "40,000 two-byte characters give the field error "Keep it to 64 KiB.",
focus stays in the field and nothing is sent."

Not tested here: a Codex fork against the real Codex service (the reviewer's run used a local stub
API); Antigravity, refused by design; the UI against a real server (the e2e runs against the
page's mock, whose fork route mirrors the server's refusals).

Staging (request `e4654709`, revision `46b67ad`): gate_failed, run `ab04d403`, test (1/2):
`yulius::bin/yulius tests::the_agent_wrapper_passes_every_session_command` ("run not in [wait,
tool, security-scan, remove]"). #1715 (brokered commands) added `yulius run` "In a task's session"
without adding it to the agent's `yulius` shim (`identity::agent_yulius_shim`). The same failure is
on dev at `7dc264180` (run `72091baf`). Fixed forward here: the shim passes `run` and names it in
its usage. The test passes, and `identity::tests` (11) passes with a `run` case.
