---
yulius-task: 96eea568-73fb-4b39-8179-d2211f150f51
status: in-progress
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
- [ ] Independent review of the backend pull request
- [ ] Fork action in the task menu and on the task page, with its dialog
- [ ] Fork origin and forks shown on both task pages
- [ ] End-to-end spec of the Fork dialog and the origin shown on both pages
- [ ] Product docs and the changes log for the UI; independent review of the UI pull request

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
