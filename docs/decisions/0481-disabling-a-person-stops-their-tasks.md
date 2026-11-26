# 0481. Disabling a person stops their tasks, and their runs get no git credential

## Status

Accepted. Specified in [SPEC-675](../../.ai/specs/SPEC-675-fix-close-the-two-l4-q3-authorisation-ga.md)
(L4Q3-001, L4Q3-002); task #659, from audit L4 question Q3 (confirmed by task #653). Extends what
disabling does (SPEC-001: sessions and API tokens revoked) and the agent token of
[0114](0114-agents-hold-a-token-github-refuses-on-production.md). It keeps the rule from
[0331](0331-anyone-tidies-a-task-and-only-its-owners-consent-passes-control.md) that a disabled
owner is absent, so anyone who may work in the project takes control at once.

## Context

When a superadmin disabled a person, Yulius revoked that person's web sessions, API tokens and
cached access. Their tasks' agents kept running. Each run kept asking `/internal/git-credential`,
which issued the App's agent token to any run that was not over. Yulius's own pushes and pull
requests for such a task fall back to the App's installation token once the owner's token is
unusable (`task_github_token`); only `delivery_merge` and the merge train refuse a disabled owner.
So a run started on a disabled person's instruction could keep pushing to the project. A person's own processes were already
refused after a disable (`workspace_git_credential` checks enabled and Work). The agent path was
the one way left that acted on the disabled person's authority.

## Decision

- **A run's git credential needs its owner's standing.** The agent's credential is issued only
  while the task's owner is enabled and holds Work on the project. The agent path and the person's
  path ask one shared check, `App::git_credential_holder`, so they refuse alike. The same check
  covers an owner who only lost write access on GitHub, with no disable at all, and an owner whose
  access cannot be settled now (their GitHub sign-in can no longer be refreshed and the cached
  access is past its staleness bound): refusing is the safe side of an unknown. The task's
  terminal says why the agent's git fails once per refusal episode, which a credential issued
  again or a hand-over ends.
- **Disabling a person stops every task they own that is not resting.** That covers live runs and
  tasks that would start by themselves: queued, chained, prepared, parked and waiting for capacity.
  It uses the ordinary stop path (`stop_task_now`), waiting tasks first, on a task of its own
  after the disable and its audit are written, so a request that goes away leaves no half-done
  record. Each task is read again first and left when someone took control meanwhile. Each stop is
  audited as `task.stopped`, with the superadmin as actor and the reason `owner disabled`, and the
  task's terminal says why. A stop that fails is logged and does not undo the disable, and the
  others still stop. The credential refusal still holds for that run.
- Nothing is deleted. A stopped task keeps its worktree, branch, session and pull request. Because
  its owner is absent, anyone who may work in the project takes control and continues it. If the
  person is enabled again, they continue it themselves.

## Alternatives

- **Refuse only the credential, leave runs running.** This is the smallest change, but the run
  goes on with compute, a subscription and the agent's MCP tools, and Yulius's own push still
  publishes its branch as the App. The disable would not end what the person started. Rejected.
- **Refuse every agent call of a disabled owner's run in `agent_context`.** Hooks and MCP calls
  would fail in the middle of a turn, with no stop recorded and the process still alive. That is
  worse to read and to recover from than a stop. Rejected; the stop gives the same containment
  through the path built for it.
- **Stop only live runs.** A parked or chained task would start again by itself later, on the
  disabled person's authority. Rejected.
- **Hand the tasks to someone automatically.** Choosing a new owner is a person's decision
  (ADR-0331). Rejected; the absent-owner rule lets anyone take control.

## Consequences

- None of the person's tasks keeps running or keeps a GitHub credential. Their enabled
  automations are not covered: they still create tasks, which then get no credential (#1824).
- Disabling someone by mistake stops their work. Enabling them again and continuing each task
  recovers its work, but not every waiting task's plan: a cancelled chained task loses its link to
  its predecessor, a prepared one its start time and a queued one its place in the queue.
- The disable request takes longer by one stop per task the person owns. Stops are sequential and
  bounded by the stop path's own rounds.
- A credential request now asks the owner's access. Within the permission cache's freshness bound
  that costs nothing. After it, the request asks GitHub with the owner's token, as a person's own
  git already did. During a GitHub outage the stale-cache rule applies.
