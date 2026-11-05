# 0270. A code edit waits for the task's spec Plan

## Status

Accepted. Specified in [SPEC-426](../../.ai/specs/SPEC-426-yulius-in-repo-when-the-org-doesnt-have.md);
task #408. Extends the discipline gate of SPEC-002 (DSC-*, ADR-07) and DSC-013.

## Context

A goal task in a project on a free private GitHub organisation worked for a long time without a
spec, so without steps ("The agent has not written a plan yet"). The report put this down to the
organisation's plan. That was not the cause. Yulius's discipline gate never calls GitHub. It is
the `Stop` hook, which reads the task's workflow, the worktree's Git state and the spec file, and
it runs only when the agent ends a turn. Goal mode, the default for new tasks, keeps an agent in
one long turn, so it can audit and change code for a long time before its first `Stop`. Until
then the rule is only advice in its briefing. The only `PreToolUse` hook Yulius registered was a
local check on Bash commands.

## Decision

- Claude Code also registers `PreToolUse` for `Edit`, `MultiEdit`, `Write` and `NotebookEdit`,
  with the same `yulius hook claude PreToolUse` command. The hook client sends those calls to Yulius
  (`/hooks/claude/PreToolUse`) and fails open, like every forwarded hook, and is never held. It
  sends only the event, the tool, the agent's folder and the edited path, never the file's content.
  Bash is still checked in the sandbox and is never sent. The tool list is one constant in
  `yulius-contracts` (`EDIT_TOOLS`).
- Yulius's edit gate denies the call (`permissionDecision: deny` and a reason) when the file is
  inside the task's worktree (a relative path from the agent's `cwd`, `.` and `..` resolved
  lexically, or through links to the worktree), it is not documentation
  (the DSC-013 rule), the task needs a spec for such a change (its workflow's spec gate, or no
  workflow), and the task's spec does not exist yet or has no top-level Plan item. The reason says
  where to write the spec and what it needs. Everything else passes, and so does any failure to
  decide.
- Like the Stop gate, it gives up after three denials in a turn: the rest of that turn's edits
  pass and the task's terminal says so once. A spec Yulius cannot read, or a person telling the
  agent to go on without one, does not leave the agent stuck.
- The edit gate asks only for a Plan. Full validity is still the `Stop` gate's job, so a draft spec
  does not hold the work.

## Alternatives

- **Check at more turn ends**, for example by asking the goal evaluator to stop sooner. That
  does not stop an edit, and a turn's length is Claude Code's choice.
- **Deny every tool until a plan exists.** Reading, auditing and running tests come before a spec,
  so blocking them would stop the research the spec is built on.
- **Check Bash edits too** (`sed -i`, redirects). There is no reliable way to tell a write from a
  read in shell text. The `Stop` gate still sees whatever such edits change.
- **Use GitHub branch protection or required checks.** A free private repository cannot have
  them, and the gate has to run before the push, not after it. Yulius stays the enforcer.
- **A nudge as additional context, not a denial.** The briefing already gives that advice, and the
  report shows that advice alone is ignored.

## Consequences

- A task that needs a spec gets its Plan, and so its steps, before its first code edit, whatever
  the repository's host or plan. Documentation, the spec itself and files outside the worktree are
  never held.
- Each edit-tool call costs one loopback round trip, with three store reads (the run, the task
  and its workflow) and a read of the spec file. While the spec is missing, each denied edit also
  looks for it under the specs directory (`yulius.toml` and up to 500 candidate files), as the Stop
  gate does.
- A worktree nested inside the repository (Claude Code's `.claude/worktrees/<name>/`) is judged
  from the task's root: a non-Markdown file under its `docs/` counts as code.
- Sessions launched before this change keep their settings until they restart. An older hook
  client sends nothing, so its edits pass as before.
- An agent can still change code through Bash before it has a Plan. The `Stop` gate catches that
  at the end of the turn, as it did before.
