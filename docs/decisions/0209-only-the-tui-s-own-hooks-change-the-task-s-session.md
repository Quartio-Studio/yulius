# 0209. Only the TUI's own hooks change the task's session

## Status

Accepted. Specified in [SPEC-281](../../.ai/specs/SPEC-281-it-s-broken-one-view-is-bartek-s-and-one.md);
task #267, issue #862.

## Context

A task's session (the attempt's `provider_session_id` and its lineage in `provider_sessions`) is
what watchers read (SPEC-179), what the transcript watch measures, and what a Continue, a limit
rotation or a queued start resumes. Claude Code can change it inside one process (`/clear`,
`/resume`), so the supervisor checked every 5 s for "drift" (HRN-004): the most recently modified
`<id>.jsonl` in the worktree's Claude project directory became the task's session.

Task #266's agent ran a nested `claude` in its worktree to reproduce a scroll bug. Same `HOME`,
same cwd: its transcript landed in the same directory and was adopted. The owner's terminal still
showed the real session, but a watcher saw the nested one ("Not logged in · Please run /login"),
and because the lineage was listed by `first_seen` the next resume would have continued it too.

## Decision

- The run's `SessionStart` hook names the task's session: when its `session_id` is a valid id
  (`yulius_domain::harness::valid_session_id`, now shared with the harness paths) other than the
  attempt's, it becomes the attempt's and enters the lineage at once. The hooks are configured
  only in the run's `--settings` file, so only the TUI Yulius launched reports them. An id already
  in another task's lineage is refused, and the store's upsert never moves another task's entry.
- The supervisor's drift check reads the attempt's session from the store instead of the project
  directory, and adopts a change as before: into the lineage, a new transcript watch, and the
  session counts as taken in hand by a person.
- `provider_sessions` lists the lineage by `last_seen` (then `first_seen`): the session adopted
  last, including an earlier one adopted again, is the one watchers read and resumes continue.

## Alternatives

- **Tell the files apart by content.** An interactive nested `claude` writes the same records as
  the TUI; nothing in the transcript says which process wrote it.
- **The TUI's open file descriptors.** Claude Code opens, appends and closes; the file is not held.
- **Claude Code's `~/.claude/sessions/<pid>.json`.** Undocumented and version-dependent.
- **Give agents' nested runs another `HOME`.** Relies on every agent remembering it; one forgotten
  repro corrupts the task again.
- **Adopt from every hook event.** Every input carries a `session_id`, but `SessionStart` is the
  event Claude Code fires when the TUI's session begins or changes; others (a background agent's, a
  teammate's) are not guaranteed to carry the TUI's id.
- **Migrate stored lineages.** The rows were right; only the order read was wrong.

## Consequences

- A `claude` the agent runs in its worktree (repros, probes) cannot take over the task's session,
  watchers' view or resume.
- A session change is adopted only through hooks: a TUI whose hooks cannot reach the server (it is
  down) keeps the old session until a later `SessionStart`. Hooks are part of every Claude run.
- A restart between the hook and the next drift check keeps the new session; an agent naming
  another task's session (the sandbox's tasks share one Claude directory) changes nothing.
- Assumed, not verified: Claude Code's agent-team teammates do not run the lead's `--settings`
  hooks. If one did, its `SessionStart` would name its own session; teams are not enabled in
  Yulius runs.
- Task #266, whose TUI session was adopted again after the nested one, reads right after deploy
  without a migration.
