# 0086. A lost onboarding task returns the sandbox to pending

## Status

Accepted. Specified in [SPEC-106](../../.ai/specs/SPEC-106-onboarding-another-repo-got-fucked-up-by.md)
(issue #425); task #91.

## Context

The studio sandbox waited in `preparing` with no onboarding task. Its task had been deleted,
which clears `sandboxes.onboarding_task_id`. An archived onboarding task left the page's task
list, so the gate read it as missing. Nothing moved a sandbox out of that state. The only way out
was an admin's `POST …/onboarding/skip`, which skips preparation rather than doing it.

The same task's session had opened on Claude Code's folder-trust prompt. Claude Code 2.1.292
looks the trust of a linked worktree up under its main repository when `.claude/settings.json`
pre-approves tools, and Yulius trusted only the worktree.

## Decision

- A `preparing` sandbox whose onboarding task no longer exists returns to `pending` (audited as
  `system`). Start onboarding then creates and starts a new onboarding task, as for a sandbox
  nobody has started (ADR-0077).
- While onboarding blocks the sandbox, archiving its onboarding task is refused (409). The message
  names the ways out: continue the task, or delete it to start over. An onboarding task archived
  before this change is restored to the list.
- These repairs run in `refresh_onboarding` (on task close, task delete and the gates), and once
  for every sandbox at server start (`heal_onboarding`). That is how sandboxes stuck before the
  fix heal with the deploy, with no migration.
- Each launch trusts the worktree's main repository too, read from the worktree's `.git` file and
  `commondir`. Yulius owns both (same sandbox), so trusting the clone grants nothing it had not
  decided already for the worktree.

## Alternatives

- **Skip onboarding by itself when its task is lost.** The sandbox would be unblocked, but never
  prepared: tasks would meet missing toolchains and documents, the reason onboarding is mandatory
  (ADR-0076).
- **Restart the onboarding task by itself.** It would launch a paid agent without a person, and
  onboarding is started by a person in the browser (SPEC-051 OBD-002).
- **Allow archiving and un-archive at once.** The archive request would answer success and then
  undo itself, a toast that lies.
- **A data migration for the stuck sandboxes.** It runs once. The startup sweep also repairs
  states that a crash or an old binary leaves later, and it costs one read per sandbox at start.
- **Answer the trust prompt by keystrokes.** It depends on the dialog's wording and default
  answer, which changed between Claude Code releases. Yulius instead waits for a person when the
  prompt still appears (SPEC-106 OTR-002).

## Consequences

- A person who deletes the onboarding task sees "Onboard <project> first" with Start onboarding,
  not a dead end.
- The sidebar's Archive of an onboarding task in progress answers with an error toast that says
  what to do.
- Tasks chained after a deleted onboarding task need a person, as for any deleted predecessor
  (TSK-012). New tasks wait for the new onboarding task.
- Every task in a worktree of a repository whose `.claude/settings.json` pre-approves tools
  starts without the trust prompt, not only onboarding.
